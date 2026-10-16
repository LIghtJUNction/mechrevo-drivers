// Actual generated TCC classes; hardware/native and daemon inputs are mocks.
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const ts=require(process.argv[2]),root=process.argv[3],group=process.argv[4],kind=process.argv[5]||'ts';
const logs=[],events=[];let auto=()=>true,enable=()=>true;
const io={setFansAuto:()=>{events.push('auto');return auto();},setEnableModeSet:v=>{events.push('enable:'+v);return enable(v);},getFansOffAvailable:()=>true,getFansMinSpeed:()=>20,getNumberFans:()=>2,wmiAvailable:()=>true,getModuleInfo:()=>true,
 getFanTemperature:(i,out)=>{out.value=i<2?51+i:-1;return i<2;},getFanSpeedPercent:(i,out)=>{out.value=35+i;return true;},setFanSpeedPercent:(i,value)=>{events.push('write:'+i+':'+value);return true;}};
function load(name,imports){
 const s=fs.readFileSync(kind==='js'?path.join(root,'service-app/classes',name+'.js'):path.join(root,'src/service-app/classes',name+'.ts'),'utf8');
 const js=kind==='js'?s:ts.transpileModule(s,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
 const m={exports:{}};vm.runInNewContext(js,{module:m,exports:m.exports,require:n=>imports[n]||{},console:{log:(...s)=>logs.push(s.join(' ')),error:(...s)=>logs.push(s.join(' '))},setTimeout,clearTimeout,Date,Map,Promise,AggregateError});return m.exports[name];
}
const DaemonWorker=load('DaemonWorker',{});
const logic=()=>({reportTemperature:()=>{},getSpeedPercent:()=>70});
class FanBase {constructor(tccd){this.tccd=tccd;this.fans=new Map();}setFan(index){this.fans.set(index,logic());}async getFans(){return this.fans;}}
const Fan=load('FanControlTuxedoIO',{'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io},'./FanControlBaseClass':{FanControlBaseClass:FanBase},'./FanControlLogic':{FAN_LOGIC:{CPU:'cpu',GPU:'gpu'}}});
const Worker=load('FanControlWorker',{'./DaemonWorker':{DaemonWorker},'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io},'./FanControlTuxedoIO':{FanControlTuxedoIO:Fan},'../../common/models/IFanData':{FanData:class {constructor(time,speed,temp){this.time=time;this.speed=speed;this.temp=temp;}}}});
function reset(){events.length=logs.length=0;auto=()=>true;enable=()=>true;}
function setup(enabled=true){const tccd={settings:{fanControlEnabled:enabled},dbusData:{sensorDataCollectionStatus:true}},fan=new Fan(tccd),worker=new Worker(tccd,{});worker.fanApi=fan;worker.fanReadAvailable=worker.fanWriteAvailable=true;worker.fanCheckCounter=Number.MAX_VALUE;worker.activeProfile={id:'new',fan:{fanProfile:'Balanced'}};worker.previousProfile={id:'old'};worker.setFanProfile=async()=>{};return{tccd,fan,worker};}
const next=()=>new Promise(setImmediate);
function gate(){let release;return {promise:new Promise(r=>release=r),release:()=>release()};}
async function prepare(){const x=setup();await x.worker.initializeFanControlApi(true);events.length=0;return x;}
(async()=>{
if(group==='false-throw'){
 for(const first of ['false','throw'])for(const second of ['true','false','throw']){
  reset();const {fan}=setup(false),firstError=new Error('auto transport'),secondError=new Error('disable transport');
  auto=()=>{if(first==='throw')throw firstError;return false;};enable=()=>{if(second==='throw')throw secondError;return second==='true';};
  const begin=logs.length;let failure;try{await fan.initFanControl(true,false);}catch(e){failure=e;}
  assert(failure&&failure.errors,'false/throw cleanup must reject with independent errors');
  assert.equal(failure.errors.length,second==='true'?1:2);if(first==='throw')assert.equal(failure.errors[0],firstError);
  assert.deepEqual(events,['auto','enable:false']);assert(!logs.slice(begin).some(x=>x.includes('Automatic control request accepted')||x.includes('Enabling automatic')));
  auto=()=>true;enable=()=>true;await fan.initFanControl(true,false);assert.equal(fan.cleanupRequired,false);
 }
 reset();const x=setup(false);auto=()=>true;enable=()=>false;await assert.rejects(x.fan.initFanControl(true,false),/cleanup incomplete/);assert.deepEqual(events,['auto','enable:false']);
 for(const outcome of ['false','throw']){reset();const{fan}=setup();enable=()=>{if(outcome==='throw')throw new Error('enable transport');return false;};await assert.rejects(fan.initFanControl(true,true),/enable/);assert.deepEqual(events,['enable:true']);assert.equal(fan.cleanupRequired,true);}
 reset();const {fan}=setup();await fan.initFanControl(false,true);await fan.exit();assert.deepEqual(events,[]);
 await fan.initFanControl(true,true);events.length=0;await fan.initFanControl(false,false);assert.deepEqual(events,['auto','enable:false']);assert.equal(fan.cleanupRequired,false);events.length=0;await fan.exit();assert.deepEqual(events,[]);
}else if(group==='mapping-retry'){
 reset();const {worker}=setup(false);worker.mapStatus=true;auto=()=>false;await assert.rejects(worker.initializeFanControlApi(false),/cleanup incomplete/);assert.equal(worker.mapStatus,false);
 auto=()=>true;await worker.work();assert.equal(worker.mapStatus,true);assert(!events.some(x=>x.startsWith('write:')));
 reset();const x=setup();enable=()=>false;await assert.rejects(x.worker.initializeFanControlApi(true),/enable/);assert.equal(x.worker.mapStatus,false);assert(!events.some(x=>x.startsWith('write:')));
 enable=()=>true;await x.worker.work();assert.equal(x.worker.mapStatus,true);assert(events.some(x=>x.startsWith('write:')));
}else if(group==='exit-drain-reopen'){
 reset();const {worker}=await prepare(),wait=gate();let entered=false;
 worker.checkFanApiAvailable=async()=>{entered=true;await wait.promise;};const tick=worker.work();await next();assert(entered);
 let ended=false;const exit=worker.exit().then(()=>ended=true);await next();assert.equal(ended,false);assert(!events.includes('auto'));
 await assert.rejects(worker.start(),/exit.*progress/);assert.equal(worker.previousProfile.id,'old');await worker.work();assert.equal(worker.previousProfile.id,'old');
 wait.release();await Promise.all([tick,exit]);assert(ended);const i=events.indexOf('auto');assert(i>=0);assert(events.some(x=>x.startsWith('write:')));assert(events.every((x,j)=>!x.startsWith('write:')||j<i));
 const old=events.slice(),profile=worker.previousProfile;await worker.work();assert.deepEqual(events,old);assert.equal(worker.previousProfile,profile);
 worker.checkFanApiAvailable=async()=>{};await worker.start();await worker.work();assert(events.slice(i+1).some(x=>x==='enable:true'));assert(events.slice(i+1).some(x=>x.startsWith('write:')));
}else if(group==='exit-failed-retry'){
 reset();const {worker}=await prepare();auto=()=>false;await assert.rejects(worker.exit(),/cleanup incomplete/);assert.deepEqual(events,['auto','enable:false']);
 await assert.rejects(worker.start(),/cleanup.*unconfirmed/);const old=events.slice();await worker.work();assert.deepEqual(events,old);
 auto=()=>true;await worker.exit();await worker.start();await worker.work();assert(events.some(x=>x.startsWith('write:')));
 reset();const x=setup();x.worker.fanApi=undefined;await x.worker.exit();assert.deepEqual(events,[]);
}else if(group==='start-drain-busy-profile'){
 reset();const {worker}=await prepare(),wait=gate();worker.checkFanApiAvailable=async()=>await wait.promise;const tick=worker.work();await next();
 const original=worker.onStart.bind(worker);worker.onStart=async()=>{events.push('start');await original();};const start=worker.start();await next();assert(!events.includes('start'));
 await worker.work();assert.equal(worker.previousProfile.id,'old');wait.release();await Promise.all([tick,start]);const i=events.indexOf('start');assert(i>=0);assert(events.every((x,j)=>!x.startsWith('write:')||j<i));
 worker.checkFanApiAvailable=async()=>{};await worker.work();assert.equal(worker.mapStatus,true);
}else if(group==='start-failure-queue'){
 reset();const {worker}=setup();worker.onStart=async()=>await worker.initializeFanControlApi(true);enable=()=>false;await assert.rejects(worker.start(),/enable/);assert.equal(worker.mapStatus,false);assert.equal(worker.lifecyclePending,0);
 enable=()=>true;await worker.start();assert.equal(worker.mapStatus,true);assert.equal(worker.lifecyclePending,0);await worker.work();assert(events.some(x=>x.startsWith('write:')));
}else if(group==='exit-waits-start'){
 reset();const {worker}=setup(),wait=gate();worker.onStart=async()=>{events.push('start-enter');await wait.promise;await worker.initializeFanControlApi(true);events.push('start-done');};
 const start=worker.start();await next();let ended=false;const exit=worker.exit().then(()=>ended=true);await next();assert(!ended&&!events.includes('auto'));await worker.work();assert.equal(worker.previousProfile.id,'old');wait.release();await Promise.all([start,exit]);assert(events.indexOf('auto')>events.indexOf('start-done'));assert.equal(worker.closing,true);
}else if(group==='queued-start-cancel'){
 for(const pending of [false,true]){
  reset();const {worker}=await prepare(),wait=gate();worker.checkFanApiAvailable=async()=>await wait.promise;
  let commits=0,previous=worker.previousProfile;Object.defineProperty(worker,'previousProfile',{get:()=>previous,set:v=>{previous=v;++commits;},configurable:true});
  const original=worker.onStart.bind(worker);worker.onStart=async()=>{events.push('start-called');await original();};
  const tick=worker.work();await next();let rejection,wrongSuccess=false;const start=worker.start().then(()=>wrongSuccess=true,e=>rejection=e);
  if(pending)await next();const exit=worker.exit();await next();assert(!events.includes('auto')&&!events.includes('start-called'));
  wait.release();await Promise.all([tick,start,exit]);assert(rejection&&/cancelled/.test(rejection.message));assert.equal(wrongSuccess,false);assert(!events.includes('start-called'));assert.equal(commits,2); /* Existing base commits once for work, once for exit; never for cancelled start. */
  const autoIndex=events.indexOf('auto');assert(autoIndex>=0);assert(events.every((x,j)=>!x.startsWith('write:')||j<autoIndex));
 }
}else if(group==='owned-capability-loss'){
 reset();const {worker,fan}=await prepare();worker.tccd.settings.fanControlEnabled=false;worker.fanWriteAvailable=false;worker.mapStatus=true;
 auto=()=>false;enable=v=>{if(!v)throw new Error('disable transport');return true;};await assert.rejects(worker.initializeFanControlApi(false),e=>Array.isArray(e.errors)&&e.errors.length===2);assert.equal(worker.mapStatus,false);assert.equal(fan.cleanupRequired,true);assert.deepEqual(events,['auto','enable:false']);
 auto=()=>true;enable=()=>true;await worker.work();assert.equal(worker.mapStatus,true);assert.equal(fan.cleanupRequired,false);assert(!events.some(x=>x.startsWith('write:')));assert.deepEqual(events,['auto','enable:false','auto','enable:false']);
 reset();const clean=setup(false);clean.worker.fanWriteAvailable=false;await clean.worker.initializeFanControlApi(false);await clean.worker.exit();assert.deepEqual(events,[]);assert.equal(clean.worker.mapStatus,true);
}else if(group==='restart-start-errors'){
 for(const stage of ['enable','auto','disable'])for(const outcome of ['false','throw']){
  reset();const {worker,fan,tccd}=await prepare();await worker.exit();events.length=0;
  tccd.settings.fanControlEnabled=stage==='enable';worker.updateProfile({id:'restart',fan:{fanProfile:'Balanced'}});const previous=worker.previousProfile;
  const fail=()=>{if(outcome==='throw')throw new Error(stage+' transport');return false;};
  auto=stage==='auto'?fail:()=>true;enable=stage==='enable'?v=>v?fail():true:stage==='disable'?v=>!v?fail():true:()=>true;
  await assert.rejects(worker.start(),/transport|mode-enable|cleanup incomplete/);assert.equal(worker.previousProfile,previous);assert.equal(worker.mapStatus,false);assert.equal(worker.fanApi,fan);assert(!events.some(x=>x.startsWith('write:')));
  auto=()=>true;enable=()=>true;await worker.start();assert.equal(worker.mapStatus,true);assert.equal(worker.previousProfile.id,'restart');
  if(tccd.settings.fanControlEnabled){await worker.work();assert(events.some(x=>x.startsWith('write:')));}else assert(!events.some(x=>x.startsWith('write:')));
 }
}else throw new Error('unknown group '+group);
console.log(JSON.stringify({group,kind,passed:true,actual_classes:true,no_daemon_or_hardware:true}));
})().catch(e=>{console.error(e);process.exitCode=1;});
