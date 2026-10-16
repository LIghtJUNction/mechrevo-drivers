// SPDX-License-Identifier: GPL-3.0-or-later
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const ts=require(process.argv[2]),root=process.argv[3],kind=process.argv[4]||'ts';const logs=[];let ok=false,writes=0;
const io={getFanSpeedPercent:(i,out)=>{out.value=73;return ok;},getFanTemperature:(i,out)=>{out.value=51;return ok;},setFanSpeedPercent:()=>{writes++;return ok;}};
const modules={};
function load(name,imports) {
 const source=fs.readFileSync(kind==='js'?path.join(root,'service-app/classes',name+'.js'):path.join(root,'src/service-app/classes',name+'.ts'),'utf8');
 const js=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
 const module={exports:{}};vm.runInNewContext(js,{module,exports:module.exports,require:n=>imports[n]||{},console:{log:x=>logs.push(x),error:x=>logs.push(x)},setTimeout,clearTimeout,Date,Map,Promise});return module.exports[name];
}
const DaemonWorker=load('DaemonWorker',{});
const Fan=load('FanControlTuxedoIO',{'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io},'./FanControlBaseClass':{FanControlBaseClass:class {}}});
const Worker=load('FanControlWorker',{'./DaemonWorker':{DaemonWorker},'../../native-lib/TuxedoIOAPI':{TuxedoIOAPI:io},'./FanControlTuxedoIO':{FanControlTuxedoIO:Fan},'../../common/models/IFanData':{FanData:class {constructor(time,speed,temp){this.time=time;this.speed=speed;this.temp=temp;}}}});
(async()=>{
 const fan=Object.create(Fan.prototype);
 assert.equal(await fan.getFanSpeedPercent(0),-1);assert.equal(await fan.getFanTemperature(0),-1);
 await assert.rejects(fan.writeFanSpeed(0,73),/write.*failed/);
 ok=true;assert.equal(await fan.getFanSpeedPercent(0),73);assert.equal(await fan.getFanTemperature(0),51);await fan.writeFanSpeed(0,73);
 for(const value of [0,100]){io.getFanSpeedPercent=(i,out)=>{out.value=value;return true;};assert.equal(await fan.getFanSpeedPercent(0),value);}
 const worker=Object.create(Worker.prototype);worker.tccd={settings:{fanControlEnabled:true},dbusData:{sensorDataCollectionStatus:true}};
 worker.fanApi=fan;worker.previousTempValues=new Map([[0,37]]);worker.isUniwill=false;worker.mapStatus=true;worker.fanReadAvailable=true;worker.fanCheckCounter=20;
 worker.checkFanApiAvailable=async()=>{};worker.checkFanControlAvailable=async()=>{};worker.updateFanControlValues=async()=>worker.writeFanSpeed({},0,73);
 writes=0;ok=false;await worker.work();assert.equal(writes,1);assert.equal(worker.previousTempValues.get(0),37);assert(logs.some(x=>String(x).includes('onWork failed')));
 ok=true;await worker.work();assert.equal(writes,2);assert.equal(worker.previousTempValues.get(0),73);await worker.work();assert.equal(writes,2);
 io.getFanSpeedPercent=()=>{throw new Error('native throw');};await assert.rejects(fan.getFanSpeedPercent(0),/native throw/);
 // Full actual two-fan batching: one failed write, one delayed successful write.
 const parallelFan=Object.create(Fan.prototype),parallel=Object.create(Worker.prototype);
 parallel.tccd={settings:{fanControlEnabled:true},dbusData:{sensorDataCollectionStatus:true}};
 parallel.dbusData={fans:Array.from({length:3},()=>({temp:{timestamp:-1,temp:-1},speed:{timestamp:-1,speed:-1}}))};
 parallel.fanApi=parallelFan;parallel.previousTempValues=new Map([[0,37],[1,62]]);parallel.isUniwill=false;parallel.mapStatus=true;parallel.fanReadAvailable=true;parallel.fanWriteAvailable=true;parallel.fanCheckCounter=20;parallel.nrTempsAvailable=2;
 parallel.activeProfile={id:'new'};parallel.previousProfile={id:'old'};
 parallel.checkFanApiAvailable=async()=>{};parallel.checkFanControlAvailable=async()=>{};
 const logic=()=>({reportTemperature:()=>{},getSpeedPercent:()=>80});parallelFan.getFans=async()=>new Map([[1,logic()],[2,logic()]]);
 io.getFanTemperature=(i,out)=>{out.value=51+i;return true;};io.getFanSpeedPercent=(i,out)=>{out.value=i===0?37:62;return true;};
 const parallelWrites=[];io.setFanSpeedPercent=(i,speed)=>{parallelWrites.push([i,speed]);return i===1;};
 let release,delay=true;const gate=new Promise(r=>release=r);
 parallelFan.writeFanSpeed=async(i,speed)=>{await Fan.prototype.writeFanSpeed.call(parallelFan,i,speed);if(i===1&&delay)await gate;};
 let settled=false;const tick=parallel.work().then(()=>settled=true);await new Promise(setImmediate);
 assert.deepEqual(parallelWrites,[[0,80],[1,80]]);assert.equal(settled,false);
 await parallel.work();assert.deepEqual(parallelWrites,[[0,80],[1,80]]);assert.equal(parallel.previousProfile.id,'old');
 assert.equal(parallel.dbusData.fans[0].speed.speed,37);assert.equal(parallel.previousTempValues.get(0),37);
 release();delay=false;await tick;assert.equal(parallel.dbusData.fans[1].speed.speed,62);assert.equal(parallel.previousTempValues.get(1),80);assert.equal(parallel.previousProfile.id,'new');
 await parallel.work();assert.deepEqual(parallelWrites,[[0,80],[1,80],[0,80]]);assert.equal(parallel.dbusData.fans[0].speed.speed,37);
 // A failed actual read publishes the existing invalid sentinel, not old valid telemetry.
 io.getFanSpeedPercent=(i,out)=>{if(i===0)throw new Error('read failed');out.value=62;return true;};
 await parallel.work();assert.equal(parallel.dbusData.fans[0].speed.speed,-1);assert(parallel.dbusData.fans[0].speed.timestamp>=0);assert.equal(parallel.dbusData.fans[1].speed.speed,62);
 assert(logs.some(x=>String(x).includes('read failed')&&String(x).includes('write')));
 io.getFanTemperature=(i,out)=>{out.value=51;return i!==0;};await parallel.work();assert.equal(parallel.dbusData.fans[0].temp.temp,-1);assert.equal(parallel.dbusData.fans[0].speed.speed,-1);
 // Both native writes fail: all fan telemetry settles and the next timer tick still runs.
 io.getFanTemperature=(i,out)=>{out.value=51;return true;};io.getFanSpeedPercent=(i,out)=>{out.value=i===0?37:62;return true;};io.setFanSpeedPercent=(i,speed)=>{parallelWrites.push([i,speed]);return false;};
 parallel.isUniwill=true;const previousCount=parallelWrites.length;await parallel.work();await parallel.work();assert.equal(parallelWrites.length,previousCount+4);assert.equal(parallel.previousTempValues.get(0),37);assert.equal(parallel.previousTempValues.get(1),80);
 assert.equal(parallel.dbusData.fans[0].speed.speed,37);assert.equal(parallel.dbusData.fans[1].speed.speed,62);assert(logs.some(x=>String(x).includes('index 0')&&String(x).includes('index 1')));
 console.log(JSON.stringify({actual_generated_service_methods:true,failed_reads_invalid:true,failed_write_rejects:true,worker_cache_retry_and_outer_catch:true,two_fans_settled_actual_telemetry:true,timer_guard_and_next_tick_retry:true,no_daemon_or_hardware:true}));
})().catch(e=>{console.error(e);process.exitCode=1;});
