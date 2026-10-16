// SPDX-License-Identifier: GPL-3.0-or-later
// Actual worker, IO service, FanData, D-Bus getter and consumer; no daemon/native/hardware.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const [tsPath,root,group,kind='ts',originalRoot=root]=process.argv.slice(2),ts=require(tsPath);
const io={getFanTemperature:(i,out)=>{out.value=51+i;return true;},getFanSpeedPercent:(i,out)=>{out.value=37+i;return true;},setFanSpeedPercent:()=>true};
const compile=s=>ts.transpileModule(s,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022,experimentalDecorators:true}}).outputText;
function load(name,rel,imports={}){
 const source=fs.readFileSync(path.join(root,kind==='js'?rel.replace(/\.ts$/,'.js'):'src/'+rel),'utf8');
 const module={exports:{}};vm.runInNewContext(kind==='js'?source:compile(source),{module,exports:module.exports,require:n=>imports[n]||{},console:{log(){},warn(){},error(){}},Map,Promise,Date,Number,Error,AggregateError,String,setTimeout,clearTimeout});return module.exports[name];
}
const Daemon=load('DaemonWorker','service-app/classes/DaemonWorker.ts');
const Base=load('FanControlBaseClass','service-app/classes/FanControlBaseClass.ts',{'./DaemonWorker':{DaemonWorker:Daemon}});
const Fan=load('FanControlTuxedoIO','service-app/classes/FanControlTuxedoIO.ts',{'./FanControlBaseClass':{FanControlBaseClass:Base},'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io}});
const FanData=load('FanData','common/models/IFanData.ts');
const Worker=load('FanControlWorker','service-app/classes/FanControlWorker.ts',{'./DaemonWorker':{DaemonWorker:Daemon},'./FanControlTuxedoIO':{FanControlTuxedoIO:Fan},'../../common/models/IFanData':{FanData},'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io}});
const Consumer=load('CompatibilityService','ng-app/app/compatibility.service.ts',{'@angular/core':{Injectable:()=>x=>x}});
// Extract the actual API getter without constructing the service or exporting a fake getter.
const getterSource=fs.readFileSync(path.join(originalRoot,'src/service-app/classes/TccDBusInterface.ts'),'utf8');
const ast=ts.createSourceFile('dbus.ts',getterSource,ts.ScriptTarget.Latest,true);
const cls=ast.statements.find(n=>ts.isClassDeclaration(n)&&n.name.text==='TccDBusInterface');
const method=cls.members.find(n=>n.name?.getText(ast)==='GetFanDataJSON');assert(method,'actual published getter');
const moduleGetter={exports:{}};vm.runInNewContext(compile('export class Getter { '+method.getText(ast)+' }'),{module:moduleGetter,exports:moduleGetter.exports});
function payload(w){return JSON.parse(moduleGetter.exports.Getter.prototype.GetFanDataJSON.call({data:w.tccd.dbusData}));}
function consumer(w){const c=Object.create(Consumer.prototype);c.tccDbus={fanData:{value:payload(w)}};return c;}
function make(count){
 const w=Object.create(Worker.prototype);w.tccd={settings:{fanControlEnabled:true},dbusData:{sensorDataCollectionStatus:true}};
 w.dbusData={fans:Array.from({length:3},()=>({temp:{timestamp:-1,temp:-1},speed:{timestamp:-1,speed:-1}}))};
 w.previousTempValues=new Map();w.isUniwill=true;w.fanWriteAvailable=true;w.nrTempsAvailable=count;
 w.fanApi=Object.create(Fan.prototype);w.fans=new Map(Array.from({length:count},(_,i)=>[i+1,{reportTemperature(){},getSpeedPercent:()=>80+i}]));return w;
}
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
(async()=>{
 const count=group==='single'?1:2,w=make(count),writes=[];io.setFanSpeedPercent=(i,v)=>{writes.push([i,v]);return true;};
 await w.setFansWithMaxSpeed(w.fans);assert.equal(writes.length,count);assert.equal(consumer(w).hasCpuTemp,true);assert.equal(consumer(w).hasCpuFan,true);
 if(count===2){assert.equal(consumer(w).hasDGpuTemp,true);assert.equal(consumer(w).hasDGpuFan,true);}
 if(group==='normal'){await w.setFansWithMaxSpeed(w.fans);assert.equal(writes.length,4);assert.equal(payload(w).cpu.temp.data,51);return;}
 writes.length=0;
 const first=group==='undefined'?undefined:new Error('CPU temperature read failed'),second=new Error('GPU temperature read failed');
 io.getFanTemperature=(i,out)=>{if(i===0)throw first;if(group==='both')throw second;out.value=52;return true;};
 const readGate=deferred(),publishGate=deferred();let secondReadSettled=false,publishCalls=[];
 const realRead=w.fanApi.getFanTemperature.bind(w.fanApi);
 if(group==='settled')w.fanApi.getFanTemperature=async i=>{if(i===1){await readGate.promise;secondReadSettled=true;}return realRead(i);};
 const realPublish=w.setFanDbusData.bind(w),publicationError=new Error('feedback publication failed');
 w.setFanDbusData=async(i,t,s)=>{publishCalls.push(i);if(group==='settled'&&i===1)await publishGate.promise;if(group==='publish-failure'&&i===0)throw publicationError;return realPublish(i,t,s);};
 let settled=false,caught;const pending=w.setFansWithMaxSpeed(w.fans).then(()=>{settled=true;throw Error('expected temperature rejection');},e=>{settled=true;caught=e;});
 if(group==='settled'){
  await new Promise(setImmediate);assert.equal(settled,false);assert.equal(publishCalls.length,0,'all reads finish before invalidation');assert.equal(writes.length,0);
  readGate.resolve();await new Promise(setImmediate);assert.equal(secondReadSettled,true);assert.deepEqual(publishCalls,[0,1],'all round fan invalidations start after reads settle');assert.equal(settled,false,'all invalidations settle before error propagation');publishGate.resolve();
 }
 await pending;assert.equal(writes.length,0,'failed temperature round must abort before every fan write');
 const p=payload(w),c=consumer(w);
 if(group!=='publish-failure'){assert.equal(c.hasCpuTemp,false,'actual consumer must not accept prior temperature after a failed round');assert.equal(c.hasCpuFan,false,'actual consumer must not accept prior speed after a failed round');assert.equal(p.cpu.temp.data,-1);assert.equal(p.cpu.speed.data,-1);}
 if(count===2){assert.equal(c.hasDGpuTemp,false,'actual GPU consumer must reject aborted round feedback');assert.equal(c.hasDGpuFan,false);assert.equal(p.gpu1.temp.data,-1);assert.equal(p.gpu1.speed.data,-1);}
 assert.deepEqual(publishCalls,Array.from({length:count},(_,i)=>i),'every round fan invalidation attempted');
 if(group==='both'||group==='publish-failure'){
  assert(caught instanceof AggregateError);assert.equal(caught.errors[0],first);assert.equal(caught.errors[1],group==='both'?second:publicationError);
  assert.match(String(caught),/CPU temperature read failed/);assert.match(String(caught),group==='both'?/GPU temperature read failed/:/feedback publication failed/);
 }else assert.equal(caught,first,'original thrown identity, including undefined, must survive');
 // A later successful round can publish fresh data again.
 io.getFanTemperature=(i,out)=>{out.value=60+i;return true;};w.setFanDbusData=realPublish;
 await w.setFansWithMaxSpeed(w.fans);assert.equal(payload(w).cpu.temp.data,60);assert.equal(consumer(w).hasCpuTemp,true);
 console.log(JSON.stringify({group,kind,actual_consumer:true,actual_published_getter:true,temperature_throw_invalidates_before_write:true,no_hardware:true}));
})().catch(e=>{console.error(e);process.exitCode=1;});
