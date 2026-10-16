// SPDX-License-Identifier: GPL-3.0-or-later
const assert=require('node:assert/strict');const api=require(process.argv[2]);
process.env.TCC_MOCK_SUCCESS='0';process.env.TCC_MOCK_RAW='146';
for(const fn of ['getFanSpeedPercent','getFanTemperature']) {
 for(const initial of [-1,73,-777]) {const out={value:initial};assert.equal(api[fn](0,out),false);assert.equal(out.value,initial);}
}
for(const index of [-1,2,99]){const out={value:-777};assert.equal(api.getFanSpeedPercent(index,out),false);assert.equal(out.value,-777);}
assert.equal(api.setFanSpeedPercent(0,73),false);
process.env.TCC_MOCK_SUCCESS='1';
for(const [raw,expected] of [[0,0],[146,73],[200,100]]) {process.env.TCC_MOCK_RAW=String(raw);const out={value:-1};assert.equal(api.getFanSpeedPercent(0,out),true);assert.equal(out.value,expected);}
process.env.TCC_MOCK_RAW='51';{const out={value:-1};assert.equal(api.getFanTemperature(0,out),true);assert.equal(out.value,51);}
process.env.TCC_MOCK_RAW='0';{const out={value:-777};assert.equal(api.getFanTemperature(0,out),false);assert.equal(out.value,-777);}
assert.equal(api.setFanSpeedPercent(0,73),true);
process.env.TCC_MOCK_OPEN='0';{const out={value:-777};assert.equal(api.getFanSpeedPercent(0,out),false);assert.equal(out.value,-777);}
assert.throws(()=>api.getFanSpeedPercent('bad',{}));assert.throws(()=>api.getFanTemperature(0,null));
console.log(JSON.stringify({actual_native_addon:true,hardware_syscalls_mocked:true,read_failure_output_preserved:true,success_and_failure_cases:true}));
