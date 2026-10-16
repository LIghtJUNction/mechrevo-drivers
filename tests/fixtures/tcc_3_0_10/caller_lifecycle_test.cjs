// Private actual-caller integration harness: no daemon constructor, native IO or timers.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),crypto=require('node:crypto');
const [tsPath,candidate,frozen,group,kind='ts']=process.argv.slice(2),ts=require(tsPath);
const logs=[],events=[],sources={};let enableFailure=false,autoFailure=false,settingsFailure=false;
const signalCallbacks=new Map(),exitCodes=[],exitSentinel=new Error('stub process.exit sentinel');
const selectedProcessRoutes=group.startsWith('signal-')||group==='fatal-error';
const processMock=new Proxy({},{get(_target,key){if(selectedProcessRoutes&&key==='on')return(name,handler)=>signalCallbacks.set(name,handler);if(selectedProcessRoutes&&key==='exit')return code=>{exitCodes.push(code);throw exitSentinel;};throw Error('process access prohibited: '+String(key));}});
const io={setFansAuto(){events.push('auto');return !autoFailure;},setEnableModeSet(v){events.push('enable:'+v);return !(v&&enableFailure);},getFansOffAvailable:()=>true,getFansMinSpeed:()=>20,getNumberFans:()=>2,wmiAvailable:()=>true,getModuleInfo(out){out.activeInterface='uniwill';return true;},getFanTemperature(i,out){out.value=i<2?60:-1;return i<2;},getFanSpeedPercent(i,out){out.value=40;return true;},setFanSpeedPercent(i,v){events.push('write:'+i+':'+v);return true;}};
function load(name,imports={}){
 const rel='src/service-app/classes/'+name+'.ts';const file=kind==='js'?path.join(candidate,'service-app/classes',name+'.js'):(fs.existsSync(path.join(candidate,rel))?path.join(candidate,rel):path.join(frozen,rel));
 const source=fs.readFileSync(file,'utf8');sources[name]={file,sha256:crypto.createHash('sha256').update(source).digest('hex')};
 const result=kind==='js'?{outputText:source,diagnostics:[]}:ts.transpileModule(source,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}});
 assert(!result.diagnostics.some(d=>d.category===ts.DiagnosticCategory.Error),'TS transpile diagnostics: '+name);
 const m={exports:{}};
 vm.runInNewContext(result.outputText,{module:m,exports:m.exports,require:n=>imports[n]||{},console:{log:(...a)=>logs.push(a.join(' ')),error:(...a)=>logs.push(a.join(' '))},Map,Promise,AggregateError,Date,clearInterval:t=>events.push('clear-timer:'+t),setTimeout:()=>{throw Error('daemon timer prohibited');},setInterval:()=>{throw Error('daemon timer prohibited');},process:processMock});
 return m.exports[name];
}
const DaemonWorker=load('DaemonWorker');
class Logic{constructor(){this.speed=70;}reportTemperature(){}getSpeedPercent(){return this.speed;}async setFanProfile(profile){events.push('profile:'+profile.id);}}
const Base=load('FanControlBaseClass',{'./FanControlLogic':{FanControlLogic:Logic}});
const Fan=load('FanControlTuxedoIO',{'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io},'./FanControlBaseClass':{FanControlBaseClass:Base},'./FanControlLogic':{FAN_LOGIC:{CPU:0,GPU:1}}});
class Absent {async checkAvailable(){return [false,false];}}
const Worker=load('FanControlWorker',{'./DaemonWorker':{DaemonWorker},'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io,ModuleInfo:class {}},'./FanControlTuxedoIO':{FanControlTuxedoIO:Fan},'./FanControlTuxi':{FanControlTuxi:Absent},'./FanControlPwm':{FanControlPwm:Absent},'../../common/models/IFanData':{FanData:class {}},'./FanControlUtils':{getCurrentCustomProfile:async p=>p}});
const Daemon=load('TuxedoControlCenterDaemon',{'./SingleProcess':{SingleProcess:class {}},'node:constants':{SIGTERM:15}});
const State=load('StateSwitcherWorker',{'./DaemonWorker':{DaemonWorker},'../../common/classes/StateUtils':{determineState:()=> 'AC'}});
const profile=id=>({id,name:id,fan:{fanProfile:'Balanced',minimumFanspeed:20,maximumFanspeed:100,offsetFanspeed:0}});
function setup(){
 const daemon=Object.create(Daemon.prototype);daemon.workerLifecycleTail=Promise.resolve();daemon.workerShutdownRequested=false;daemon.workerExitOperation=undefined;daemon.activeProfile=profile('A');daemon.settings={fanControlEnabled:true,stateMap:{AC:'A'}};daemon.dbusData={sensorDataCollectionStatus:true};daemon.config={writeSettings:()=>{events.push('settings-written');if(settingsFailure)throw Error('settings write failed');}};daemon.listeners=[];
 daemon.getCurrentFanProfile=p=>({id:(p||daemon.activeProfile).id});daemon.updateDBusActiveProfileData=()=>events.push('dbus:'+daemon.activeProfile.id);daemon.setCurrentProfileById=id=>{daemon.activeProfile=profile(id);return true;};daemon.logLine=s=>logs.push(s);
 const worker=new Worker(daemon,{});worker.fanApi=new Fan(daemon);worker.fanReadAvailable=worker.fanWriteAvailable=true;worker.fanCheckCounter=Number.MAX_VALUE;
 worker.updateProfile(daemon.activeProfile);const state=new State(daemon);state.currentState='AC';state.currentStateProfileId='A';daemon.workers=[state,worker];daemon.stateWorker=state;return{daemon,worker,state};
}
const next=()=>new Promise(setImmediate);
function gate(){let release;return{promise:new Promise(r=>release=r),release:()=>release()};}
async function settled(p){return await Promise.race([p,new Promise((_,reject)=>{const t=setTimeout(()=>reject(Error('Caller lifecycle promise did not settle')),2000);t.unref();})]);}
(async()=>{
 const x=setup();await x.worker.start();events.length=0;
 if(group==='signal-clean'||group==='signal-failed'){
  for(const [signal,cleanCode] of [['SIGINT',0],['SIGTERM',15]]){
   const current=setup();await current.worker.start();await current.daemon.setupSignalHandling();
   autoFailure=group==='signal-failed';const begin=exitCodes.length;
   await assert.rejects(()=>settled(signalCallbacks.get(signal)()),error=>error===exitSentinel);
   assert.equal(exitCodes.length,begin+1);assert.equal(exitCodes.at(-1),autoFailure?1:cleanCode);
   autoFailure=false;
  }
 }else if(group==='fatal-error'){
  for(const started of [false,true]){
   const current=setup();await current.worker.start();current.daemon.started=started;events.length=0;
   await assert.rejects(()=>settled(current.daemon.catchError(new Error('fatal test cause'))),error=>error===exitSentinel);
   assert.equal(exitCodes.at(-1),1);assert.equal(events.includes('settings-written'),started);
   assert(logs.some(s=>s.includes('fatal test cause')));
  }
 }else if(group==='trigger-state-reject'){
  let resets=0,works=0;const failure=new Error('state async failure');
  x.daemon.stateWorker={reapplyProfile(){resets++;},work(){works++;return Promise.reject(failure);}};
  x.daemon.triggerStateCheck(true);await next();await next();assert.equal(resets,1);assert.equal(works,1);
  assert(logs.some(s=>s.includes('state async failure')));
  const hostile={toString(){throw Error('hostile state toString');},[Symbol.toPrimitive](){throw Error('hostile state primitive');}};
  const begin=logs.length;x.daemon.stateWorker.work=()=>Promise.reject(hostile);x.daemon.triggerStateCheck(false);
  await next();await next();assert(logs.length>begin,'Unknown rejected state check must remain observable without throwing during formatting');
 }else if(group==='profile-during-exit'){

  const wait=gate(),original=x.worker.fanApi.exit.bind(x.worker.fanApi);x.worker.fanApi.exit=async()=>{events.push('exit-enter');await wait.promise;await original();};
  const exiting=x.worker.exit();await next();x.daemon.settings.stateMap.AC='B';
  const applying=x.state.onWork();await settled(applying);assert.equal(x.state.refreshProfile,true,'Failed profile application must remain pending');
  assert(logs.some(s=>/onStart|profile|exit/i.test(s)),'Failed apply must be observable');
  wait.release();await settled(exiting);await settled(x.state.onWork());
  assert.equal(x.state.refreshProfile,false,'Successful explicit retry clears pending flag');
  assert(events.includes('profile:B'),'Actual worker must apply new profile after cleanup');
 }else if(group==='refresh-during-apply'){
  const wait=gate(),original=x.worker.fanApi.initFanControl.bind(x.worker.fanApi);let first=true;
  x.worker.fanApi.initFanControl=async(...args)=>{if(first){first=false;await wait.promise;}await original(...args);};
  x.state.reapplyProfile();const applying=x.state.onWork();await next();x.state.reapplyProfile();
  wait.release();await settled(applying);
  assert.equal(x.state.refreshProfile,true,'A new refresh requested during apply must not be cleared by the older completion');
  const applied=events.filter(s=>s.startsWith('profile:')).length;await settled(x.state.onWork());
  assert.equal(x.state.refreshProfile,false);assert(events.filter(s=>s.startsWith('profile:')).length>applied);
 }else if(group==='shutdown-during-start'){
  const wait=gate(),original=x.worker.fanApi.initFanControl.bind(x.worker.fanApi);let first=true;
  x.worker.fanApi.initFanControl=async(...args)=>{if(first){first=false;await wait.promise;}await original(...args);};
  const starting=x.daemon.startWorkers();await next();const shutting=x.daemon.onExit();await next();
  const late=x.daemon.startWorkers();wait.release();
  await settled(Promise.allSettled([starting,shutting]));assert.equal(await settled(late),false,'Late shutdown start must return false');
  const autoIndex=events.lastIndexOf('auto');assert(autoIndex>=0,'Shutdown must perform AUTO cleanup');
  assert(!events.slice(autoIndex+1).includes('enable:true'),'No queued start may reopen manual mode after cleanup');
 }else if(group==='shutdown-no-reopen'){
  await settled(x.daemon.onExit());const after=events.slice();assert.equal(await settled(x.daemon.startWorkers()),false);
  assert.deepEqual(events,after,'Daemon shutdown must not reopen manual mode');
 }else if(group==='exit-failure-retry'){
  let hostileExit=true;const hostile={toString(){throw Error('hostile toString');},[Symbol.toPrimitive](){throw Error('hostile primitive');}};
  class OtherWorker extends DaemonWorker{constructor(){super(1,'OtherWorker',x.daemon);}async onStart(){}async onWork(){}async onExit(){events.push('other-exit');if(hostileExit)throw hostile;}}
  x.daemon.workers.push(new OtherWorker());autoFailure=true;settingsFailure=true;
  assert.equal(await settled(x.daemon.onExit()),false);assert(events.includes('other-exit'));assert(events.includes('settings-written'));
  assert(logs.some(s=>s.includes('cleanup incomplete')));assert(logs.some(s=>s.includes('settings write failed')));
  assert.equal(await settled(x.daemon.startWorkers()),false);
  autoFailure=false;settingsFailure=false;hostileExit=false;assert.equal(await settled(x.daemon.onExit()),true);
  const after=events.slice();assert.equal(await settled(x.daemon.startWorkers()),false);assert.deepEqual(events,after);
 }else if(group==='hostile-start-error'){
  const hostile={toString(){throw Error('hostile toString');},[Symbol.toPrimitive](){throw Error('hostile primitive');}};
  class HostileWorker extends DaemonWorker{constructor(){super(1,'HostileWorker',x.daemon);}async onStart(){throw hostile;}async onWork(){}async onExit(){}}
  x.daemon.workers.unshift(new HostileWorker());assert.equal(await settled(x.daemon.startWorkers()),false);
  assert(events.includes('profile:A'),'Hostile exception formatting must not stop later fan initialization');assert(logs.length>0);
 }else if(group==='best-effort-workers'){
  class OtherWorker extends DaemonWorker{constructor(){super(1,'OtherWorker',x.daemon);}async onStart(){events.push('other-start');}async onWork(){}async onExit(){}}
  x.daemon.workers.push(new OtherWorker());enableFailure=true;assert.equal(await settled(x.daemon.startWorkers()),false);
  assert(events.includes('other-start'),'A failed fan worker must not stop independent workers from starting');
  assert(logs.some(s=>s.includes('mode-enable')));
 }else if(group==='concurrent-starts'){
  const wait=gate(),original=x.worker.fanApi.initFanControl.bind(x.worker.fanApi);let first=true;
  x.worker.fanApi.initFanControl=async(...args)=>{if(first){first=false;await wait.promise;}await original(...args);};
  enableFailure=true;const a=x.daemon.startWorkers();await next();const b=x.daemon.startWorkers();wait.release();
  const results=await settled(Promise.allSettled([a,b]));assert(results.every(r=>r.status==='fulfilled'&&r.value===false),'Each failed start must return false truthfully');
  assert(logs.some(s=>s.includes('mode-enable')),'Initialization failure log must expose real cause');
  enableFailure=false;assert.equal(await settled(x.daemon.startWorkers()),true);assert(events.includes('profile:A'));
 }else if(group==='restart-after-cleanup'){
  await x.worker.exit();events.length=0;await settled(x.daemon.startWorkers());
  assert(events.includes('enable:true'),'Awaiting caller start must finish actual initialization');assert(events.includes('profile:A'));
 }else if(group==='init-failure-observable'){
  enableFailure=true;assert.equal(await settled(x.daemon.startWorkers()),false);
  assert(logs.some(s=>s.includes('mode-enable')),'Daemon must retain failed init cause');
  enableFailure=false;assert.equal(await settled(x.daemon.startWorkers()),true);assert(events.includes('profile:A'));
 }else throw Error('Unknown caller group '+group);
 console.log(JSON.stringify({group,kind,passed:true,stub_exit_codes:exitCodes,actual_caller_methods:['Daemon.startWorkers','Daemon.onExit','StateSwitcher.onWork'],no_daemon_constructor:true,no_native_or_hardware:true,sources,events,logs}));
})().catch(e=>{console.error(e);process.exitCode=1;});
