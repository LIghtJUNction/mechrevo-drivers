// Calls actual prepared TypeScript IPC/webcam functions; never launches a process.
'use strict';
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const assert = require('node:assert/strict');
const [tsModule, source, workspace, scenario] = process.argv.slice(2);
const ts = require(tsModule);
const installed = '/usr/lib/tuxedo-control-center';
const cwd = path.join(workspace, 'development');
const production = ['production', 'old-sync-failure'].includes(scenario);
const asar = !['production', 'old-sync-failure', 'development'].includes(scenario);
const appPath = asar ? installed + '/app.asar' : cwd;
const handlers = {}, calls = [], writes = [], configs = [];
const app = {isPackaged: production, getAppPath: () => appPath};
const ipcMain = {on: (name, fn) => handlers[name] = fn, handle: (name, fn) => handlers[name] = fn};
function compile(relative, deps) {
 const filename = path.join(source, relative);
 const result = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
  compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}, fileName: filename,
 });
 const exports = {};
 vm.runInNewContext(result.outputText, {exports, require: name => {
  assert.ok(Object.hasOwn(deps, name), 'unexpected dependency: ' + name); return deps[name];
 }, console: {log(){}, error(){}}, Promise, process: {cwd: () => cwd}}, {filename});
 return exports;
}
const writeFs = {
 existsSync: () => true,
 writeFileSync: (...args) => {
  writes.push(args);
  if (scenario === 'write-failure') throw Object.assign(new Error('mocked configuration write failure'), {code:'EACCES'});
 },
};
const actualConfig = compile('src/common/classes/ConfigHandler.ts', {
 'node:fs': writeFs, 'node:path': path,
 '../models/DefaultProfiles': {}, '../models/profiles/LegacyProfiles': {},
 '../models/TccFanTable': {}, '../models/TccProfile': {}, '../models/TccSettings': {},
});
class ConfigHandler extends actualConfig.ConfigHandler {
 constructor(...args) {super(...args); configs.push(args);}
}
const paths = compile('src/common/classes/TccPaths.ts', {});
function checkCommand(command) {
 calls.push(command);
 const parts = command.split(' '), target = parts[1];
 const actual = target.startsWith('/usr/lib/') ? path.join(workspace, 'package', target) : target;
 fs.statSync(actual); // Tests a real private package/development file, never executes it.
 if (['launch-failure', 'old-sync-failure'].includes(scenario)) throw new Error('mocked launch failure after target exists');
 return {data: 'camera-result', error: null};
}
const childProcess = {
 execFileSync: (command, args) => Buffer.from(checkCommand([command, ...args].join(' ')).data),
 execFile: (command, args, callback) => {
  try {const result = checkCommand([command, ...args].join(' ')); callback(null, result.data, '');}
  catch (error) {callback(error, '', 'mocked failure');}
 },
 exec: (command, callback) => {
  try {const result = checkCommand(command); callback(null, result.data, '');}
  catch (error) {callback(error, '', 'mocked failure');}
 },
};
// Real utilsAPI wrappers are part of the contract. Only the OS process boundary is mocked.
const utils = compile('src/e-app/backendAPIs/utilsAPI.ts', {
 'node:child_process': childProcess, 'node:fs': fs, 'node:path': path,
 electron: {app, ipcMain}, './browserWindowsAPI': {}, './initMain': {}, './systemInfosAPI': {},
});
const common = {
 electron: {app, ipcMain},
 'node:child_process': childProcess,
 '../../common/classes/ConfigHandler': {ConfigHandler},
 '../../common/classes/TccPaths': paths,
 './utilsAPI': utils,
};
compile('src/e-app/backendAPIs/configAPI.ts', common);
const webcamKeys = new Proxy({}, {get: (_, key) => String(key)});
const webcam = compile('src/e-app/backendAPIs/webcamAPI.ts', {
 ...common,
 '../../common/models/IWebcamAPI': {WebcamAPIFunctions: webcamKeys},
 './browserWindowsAPI': {}, './initMain': {},
});
const dev = scenario === 'development';
const daemon = dev ? path.join(cwd, 'dist/tuxedo-control-center/data/service/tccd') : installed + '/data/service/tccd';
const camera = dev ? path.join(cwd, 'src/cameractrls/cameractrls.py') : installed + '/data/camera/cameractrls.py';
function create(target) {
 const actual = target.startsWith('/usr/lib/') ? path.join(workspace, 'package', target) : target;
 fs.mkdirSync(path.dirname(actual), {recursive: true}); fs.writeFileSync(actual, 'not executable: test only');
}
create(daemon); create(camera);
assert.equal(paths.TccPaths.V4L2_NAMES_FILE, installed + '/data/camera/v4l2_kernel_names.json');
assert.equal(configs.length, 2);
assert.equal(configs[0][3], paths.TccPaths.V4L2_NAMES_FILE);
if (scenario === 'missing-target') fs.unlinkSync(path.join(workspace, 'package', daemon));
if (scenario === 'missing-camera') fs.unlinkSync(path.join(workspace, 'package', camera));
(async () => {
 const success = !['missing-target', 'launch-failure', 'old-asar', 'old-sync-failure'].includes(scenario);
 const event = {};
 handlers['config-pkexec-write-custom-profiles'](event, []);
 if (scenario === 'write-failure') {
  assert.equal(event.returnValue, false);
  for (const [name,args] of [
   ['config-pkexec-write-custom-profiles-async', [{},[]]],
   ['config-pkexec-write-config-async', [{},{},[]]],
  ]) await assert.rejects(handlers[name](...args), {code:'EACCES'});
  await assert.rejects(webcam.webcamHandlers.get('writeConfig')([]), {code:'EACCES'});
  assert.equal(calls.length, 0);
  console.log(JSON.stringify({scenario, passed:true, actual_config_write_failed:true, child_execution_count:0}));
  return;
 }
 assert.equal(event.returnValue, ['old-asar','old-sync-failure'].includes(scenario) ? true : success, 'actual synchronous IPC result; original caller falsely succeeds');
 assert.equal(await handlers['config-pkexec-write-custom-profiles-async']({}, []), success);
 assert.equal(await handlers['config-pkexec-write-config-async']({}, {}, []), success);
 assert.equal(await webcam.webcamHandlers.get('writeConfig')([]), success);
 if (success) {
  handlers['config-set-active-profile']({}, 'profile-test', 'power_ac', {stateMap:{}});
  if (scenario === 'missing-camera') {
   await assert.rejects(webcam.webcamHandlers.get('getWebcamPaths')(), error => error.code === 'ENOENT' || error.error?.code === 'ENOENT');
   await assert.rejects(webcam.webcamHandlers.get('executeFilteredWebcamCtrls')('/dev/mock-only', 'x=1'), error => error === 'mocked failure');
  } else {
   assert.equal(await webcam.webcamHandlers.get('getWebcamPaths')(), 'camera-result');
   assert.equal(await webcam.webcamHandlers.get('executeFilteredWebcamCtrls')('/dev/mock-only', 'x=1'), 'camera-result');
  }
  assert.ok(calls.slice(0, 5).every(command => command.startsWith('pkexec ' + daemon + ' ')));
  assert.ok(calls.slice(5).every(command => command.startsWith('python3 ' + camera + ' ')));
 } else if (scenario !== 'old-asar') {
  assert.ok(calls.every(command => command.startsWith('pkexec ' + daemon + ' ')));
 }
 assert.ok(writes.length >= 5); // All writes are intercepted; /tmp and /etc remain untouched.
 console.log(JSON.stringify({scenario, passed:true, actual_ipc_functions:4, actual_webcam_functions:success?3:1,
  command_targets:calls, no_child_execution:true, no_system_config_write:true}));
})().catch(error => {console.error(error); process.exitCode = 1;});
