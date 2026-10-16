const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const ts=require(process.argv[2]),file=process.argv[3],kind=process.argv[4];
const src=fs.readFileSync(file,'utf8');const result=kind==='js'?{outputText:src,diagnostics:[]}:ts.transpileModule(src,{reportDiagnostics:true,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}});
assert(!result.diagnostics.some(d=>d.category===ts.DiagnosticCategory.Error));
let events=[],logs=[],auto,disable;const io={setFansAuto:()=>{events.push('AUTO');return auto();},setEnableModeSet:v=>{events.push('mode-disable');assert.equal(v,false);return disable();},getFansOffAvailable:()=>false,getFansMinSpeed:()=>0};
class Base{constructor(tccd){this.tccd=tccd;}}
const mod={exports:{}};vm.runInNewContext(result.outputText,{module:mod,exports:mod.exports,require:n=>n.includes('TuxedoIOAPI')?{TuxedoIOAPI:io}:n.includes('FanControlBaseClass')?{FanControlBaseClass:Base}:{FAN_LOGIC:{}},console:{log:(...args)=>logs.push(args.join(' '))},AggregateError});
const Fan=mod.exports.FanControlTuxedoIO;
(async()=>{let combinations=0;for(const first of ['true','false','error','primitive','badToString','badPrimitive'])for(const second of ['true','false','error','primitive','badToString','badPrimitive']){
 const group=process.argv[5]||'all';if(group==='auto-formatter'&&!(first.startsWith('bad')&&second==='true'))continue;if(group==='disable-formatter'&&!(first==='true'&&second.startsWith('bad')))continue;if(group==='both-formatters'&&!(first.startsWith('bad')&&second.startsWith('bad')))continue;
 events=[];logs=[];const fan=new Fan({dbusData:{}}),formatterError=new Error('formatter must not replace original'),bad=type=>type==='badToString'?{toString(){throw formatterError;}}:{[Symbol.toPrimitive](){throw formatterError;}},e1=first.startsWith('bad')?bad(first):new Error('auto transport distinct'),e2=second.startsWith('bad')?bad(second):{toString:()=> 'disable transport distinct'};
 auto=()=>{if(first==='error'||first.startsWith('bad'))throw e1;if(first==='primitive')throw 'auto primitive';return first==='true';};
 disable=()=>{if(second==='error'||second.startsWith('bad'))throw e2;if(second==='primitive')throw 'disable primitive';return second==='true';};
 let err,result;try{result=await fan.initFanControl(true,false);}catch(e){err=e;}
 assert.deepEqual(events,['AUTO','mode-disable']);const failed=Number(first!=='true')+Number(second!=='true');
 if(failed){assert(err instanceof AggregateError);assert.equal(err.errors.length,failed);const line=`Daemon: caught ${err}`;
  if(first!=='true'){assert(line.includes('AUTO:'));assert(line.includes(first==='error'?'auto transport distinct':first==='primitive'?'auto primitive':first.startsWith('bad')?'[unprintable error]':'automatic fan request failed'));}
  if(second!=='true'){assert(line.includes('mode-disable:'));assert(line.includes(second==='error'?'disable transport distinct':second==='primitive'?'disable primitive':second.startsWith('bad')?'[unprintable error]':'mode-disable request failed'));}
  if(first==='error'||first.startsWith('bad'))assert.equal(err.errors[0],e1);if(second==='error'||second.startsWith('bad'))assert.equal(err.errors[failed-1],e2);assert(!err.errors.includes(formatterError));assert.doesNotThrow(()=>String(err));
  if(first==='primitive')assert.equal(err.errors[0],'auto primitive');if(second==='primitive')assert.equal(err.errors[failed-1],'disable primitive');
  assert.equal(logs.length,0);assert.equal(fan.cleanupRequired,true);
  auto=disable=()=>true;assert.equal(await fan.exit(),undefined);assert.equal(fan.cleanupRequired,false);
 }else{assert.equal(err,undefined);assert.equal(result,undefined);assert.equal(fan.cleanupRequired,false);}
 assert.equal(logs.length,1);assert.equal(logs[0],'FanControlTuxedoIO: Automatic control request accepted');assert(!logs.some(s=>/RPM|mechanical|confirmed/.test(s)));++combinations;
}
 events=[];logs=[];const never=new Fan({dbusData:{}});assert.equal(await never.exit(),undefined);assert.deepEqual(events,[]);assert.deepEqual(logs,[]);
 console.log(JSON.stringify({kind,combinations,actual_class:true,formatted_error_assertions:true,original_identities:true,hardware:false}));
})().catch(e=>{console.error(e);process.exitCode=1;});
