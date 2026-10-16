// SPDX-License-Identifier: GPL-3.0-or-later
// Execute actual TS/independently-emitted JS; only external IO and sysfs properties are mocked.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const ts=require(process.argv[2]),root=process.argv[3],group=process.argv[4],kind=process.argv[5]||'ts',addon=process.argv[6];
const logs=[];const io={getFanSpeedPercent:(i,out)=>{out.value=37;return true;},getFanTemperature:(i,out)=>{out.value=51;return true;},setFanSpeedPercent:()=>true};
function load(name,rel,imports={}){
 const file=path.join(root,kind==='js'?rel.replace(/\.ts$/,'.js'):('src/'+rel));const source=fs.readFileSync(file,'utf8');
 const js=kind==='js'?source:ts.transpileModule(source,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
 const module={exports:{}};vm.runInNewContext(js,{module,exports:module.exports,require:n=>imports[n]||{},console:{log:x=>logs.push(x),warn:x=>logs.push(x),error:x=>logs.push(x)},setTimeout,clearTimeout,Date,Map,Promise,Error,AggregateError,Number,String},{filename:file});return module.exports[name];
}
const Daemon=load('DaemonWorker','service-app/classes/DaemonWorker.ts');
const Base=load('FanControlBaseClass','service-app/classes/FanControlBaseClass.ts',{'./DaemonWorker':{DaemonWorker:Daemon}});
const Fan=load('FanControlTuxedoIO','service-app/classes/FanControlTuxedoIO.ts',{'./FanControlBaseClass':{FanControlBaseClass:Base},'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io}});
const Hwmon=load('FanControlHwmon','service-app/classes/FanControlHwmon.ts',{'./FanControlBaseClass':{FanControlBaseClass:Base}});
const FanData=load('FanData','common/models/IFanData.ts');
const Worker=load('FanControlWorker','service-app/classes/FanControlWorker.ts',{'./DaemonWorker':{DaemonWorker:Daemon},'./FanControlTuxedoIO':{FanControlTuxedoIO:Fan},'./FanControlHwmon':{FanControlHwmon:Hwmon},'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io},'../../common/models/IFanData':{FanData}});
function worker(api){
 const w=Object.create(Worker.prototype);w.tccd={settings:{fanControlEnabled:true},dbusData:{sensorDataCollectionStatus:true}};
 w.dbusData={fans:Array.from({length:3},()=>({temp:{timestamp:-1,temp:-1},speed:{timestamp:-1,speed:-1}}))};
 w.fanApi=api;w.previousTempValues=new Map([[0,37],[1,62]]);w.isUniwill=true;w.fanWriteAvailable=false;
 w.fanReadAvailable=true;w.mapStatus=true;w.fanCheckCounter=20;w.closing=false;w.lifecyclePending=0;w.workInProgress=false;
 w.checkFanApiAvailable=async()=>{};w.checkFanControlAvailable=async()=>{};return w;
}
const hostile=()=>({[Symbol.toPrimitive](){throw new Error('hostile-format');}});
(async()=>{
 if(group==='aggregate-hostile'){
  const w=worker({}),first=hostile(),second=new Error('GPU fan failed');let caught;
  try{w.throwFanErrors([first,second]);}catch(e){caught=e;}
  assert(caught instanceof AggregateError,'must retain AggregateError instead of formatting failure');
  assert.equal(caught.errors[0],first);assert.equal(caught.errors[1],second);assert.match(caught.message,/unprintable error/);assert.match(caught.message,/GPU fan failed/);
  try{w.throwFanErrors([first]);}catch(e){assert.equal(e,first);}w.throwFanErrors([]);
 }else if(group==='onwork-hostile'){
  const w=worker({});w.updateFanControlValues=async()=>{throw hostile();};
  await assert.doesNotReject(w.onWork());assert(logs.some(x=>String(x).includes('[unprintable error]')));
 }else if(group==='invalid-native'){
  const f=Object.create(Fan.prototype);
  for(const value of [NaN,Infinity,-Infinity,undefined,null,'73',true,-2,-1,101,hostile()]){
   io.getFanSpeedPercent=(i,out)=>{out.value=value;return true;};await assert.rejects(f.getFanSpeedPercent(0),/invalid.*speed.*readback/i);
  }
  for(const value of [0,.5,37,100]){io.getFanSpeedPercent=(i,out)=>{out.value=value;return true;};assert.equal(await f.getFanSpeedPercent(0),value);}
  io.getFanSpeedPercent=(i,out)=>{out.value=NaN;return false;};assert.equal(await f.getFanSpeedPercent(0),-1);
 }else if(group==='invalid-worker'){
  const api={getFanSpeedPercent:async()=>37};const w=worker(api);
  for(const value of [NaN,Infinity,-Infinity,undefined,null,'73',true,-2,101,hostile()]){
   api.getFanSpeedPercent=async()=>value;await assert.rejects(w.updateFanDbusData(0,{},80,51),/invalid.*speed.*readback/i);
   assert.equal(JSON.parse(w.tccd.dbusData.fanData).cpu.speed.data,-1,'invalid reads must serialize existing unavailable sentinel');
  }
  for(const value of [-1,0,.5,37,100]){api.getFanSpeedPercent=async()=>value;await w.updateFanDbusData(0,{},80,51);assert.equal(JSON.parse(w.tccd.dbusData.fanData).cpu.speed.data,value);}
 }else if(group==='hwmon-zero'){
  const hw=Object.create(Hwmon.prototype);let input=146,max=0;hw.fanSpeedInputMap=new Map([[1,{readValueNTA:async()=>input}]]);hw.fanMaxInputMap=new Map([[1,{readValueNTA:async()=>max}]]);
  assert(Number.isNaN(await hw.getFanSpeedPercent(0)),'real unchanged upstream method must reproduce zero denominator');
  const w=worker(hw);await assert.rejects(w.updateFanDbusData(0,{},80,51),/invalid.*speed.*readback/i);assert.equal(JSON.parse(w.tccd.dbusData.fanData).cpu.speed.data,-1);
  max=200;assert.equal(await hw.getFanSpeedPercent(0),73);await w.updateFanDbusData(0,{},80,51);assert.equal(JSON.parse(w.tccd.dbusData.fanData).cpu.speed.data,73);
  input=400;assert.equal(await hw.getFanSpeedPercent(0),100,'actual hwmon clamp bounds valid positive-max ratio');
 }else if(group==='settled-hostile'){
  const w=worker({});const first=hostile(),second=new Error('second operation failed');let release;const gate=new Promise(r=>release=r);let settled=false;
  const pending=w.settleFanOperations([Promise.reject(first),gate.then(()=>{throw second;})]).then(()=>{throw Error('must reject');},e=>{settled=true;assert(e instanceof AggregateError);assert.equal(e.errors[0],first);assert.equal(e.errors[1],second);});
  await new Promise(setImmediate);assert.equal(settled,false);release();await pending;assert.equal(settled,true);
 }else if(group==='native-failure-readback'){
  assert(addon,'actual compiled native addon required');const native=require(addon);process.env.TCC_MOCK_SUCCESS='1';process.env.TCC_MOCK_OPEN='1';
  io.setFanSpeedPercent=(i,v)=>{process.env.TCC_MOCK_SUCCESS='0';try{return native.setFanSpeedPercent(i,v);}finally{process.env.TCC_MOCK_SUCCESS='1';}};
  io.getFanSpeedPercent=(i,out)=>{process.env.TCC_MOCK_RAW=String(i===0?74:124);return native.getFanSpeedPercent(i,out);};
  const f=Object.create(Fan.prototype),w=worker(f);w.fanWriteAvailable=true;
  await assert.rejects(w.updateFanDbusData(0,{},80,51),/write.*failed/);assert.equal(w.previousTempValues.get(0),37);
  assert.equal(JSON.parse(w.tccd.dbusData.fanData).cpu.speed.data,37,'valid old actual readback remains37; not requested80 or mechanical RPM');
  assert.equal(w.dbusData.fans[0].speed.speed,37);
 }else throw Error('unknown group: '+group);
 console.log(JSON.stringify({group,kind,actual_service_methods:true,actual_fan_data_model:true,no_daemon_or_hardware:true}));
})().catch(e=>{console.error(e);process.exitCode=1;});
