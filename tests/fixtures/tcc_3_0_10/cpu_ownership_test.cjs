// SPDX-License-Identifier: GPL-3.0-or-later
// Actual patched production classes; fake only CPU accessors, native imports, and selected I/O faults.
const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const crypto = require('node:crypto');
const [tsPath, source, scratch, group, workerSource = source, modules] = process.argv.slice(2);
const ts = require(tsPath), logs = [], sourceHashes = {};
const plain = x => JSON.parse(JSON.stringify(x));
let sequence = 0;
function load(name, imports = {}, root = source) {
    const filename = path.join(root, 'src/service-app/classes', name + '.ts');
    const text = fs.readFileSync(filename, 'utf8');
    sourceHashes[filename] = crypto.createHash('sha256').update(text).digest('hex');
    const out = ts.transpileModule(text, { reportDiagnostics: true,
        compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } });
    assert(!out.diagnostics.some(d => d.category === ts.DiagnosticCategory.Error), name);
    const m = { exports: {} };
    const allowed = { 'node:fs': fs, 'node:crypto': crypto, 'node:path': path, ...imports };
    vm.runInNewContext('(function(exports,require,module){' + out.outputText + '\n})', {
        Buffer, console: { error: (...s) => logs.push(s.join(' ')), log: (...s) => logs.push(s.join(' ')) },
        setTimeout, clearTimeout, ...imports.__globals,
    }, { timeout: 2000 })(m.exports, n => { assert(Object.hasOwn(allowed, n), 'blocked require: ' + n); return allowed[n]; }, m);
    return m.exports;
}
const ownerClasses = load('CpuControlOwnership');
const { CpuOwnerStore, CpuControlOwnership, CpuOwnerWriteFailure } = ownerClasses;
const { DaemonWorker } = load('DaemonWorker');
function make(io = fs) {
    const parent = path.join(scratch, 'case-' + sequence++); fs.mkdirSync(parent, { mode: 0o700 });
    const directory = path.join(parent, 'owner');
    const store = new CpuOwnerStore(directory, process.getuid(), process.getgid(), io);
    const state = { version: '3.0.10', configurationEpoch: 0, settingsDiskEpoch: 'disk-epoch-0', settingsMemory: { cpuSettingsEnabled: true, fanControlEnabled: false },
        settingsDisk: { fanControlEnabled: false, cpuSettingsEnabled: true }, profile: { id: 'office', cpu: { energyPerformancePreference: 'power' } } };
    let replays = 0, replay = () => ({ attempted: true, validation_passed: true, error: null });
    const owner = new CpuControlOwnership(() => state, () => { replays++; return replay(); }, store, s => logs.push(s));
    const file = path.join(directory, 'mechrevo-cpu-owner.json');
    return { parent, directory, file, store, owner, state, get replays() { return replays; }, set replay(v) { replay = v; } };
}
function seed(x, text, mode = 0o600) {
    fs.mkdirSync(x.directory, { mode: 0o700, recursive: true }); fs.writeFileSync(x.file, text, { mode });
}
function set(x, owner) { return x.owner.setOwner(owner, x.owner.state().token); }
function record(revision, owner) { return JSON.stringify({ schema_version: 1, revision, owner }) + '\n'; }

function worker(x) {
    const writes = [], reads = [], settings = x.state.settingsMemory;
    let throwWrite = false, throwRead = false, ignoreEpp = false;
    const value = (name, initial, available = true) => ({ value: initial, isAvailable: () => available,
        isWritable: () => true, readValue() { reads.push(name); if (throwRead) throw Error('readback failed'); return this.value; },
        readValueNT() { return this.readValue(); }, writeValue(v) { writes.push([name, v]); if (throwWrite) throw undefined; this.value = v; } });
    class CpuController {
        constructor() {
            this.cores = [0, 1].map(i => ({ coreIndex: i, online: value('online' + i, true),
                scalingDriver: value('driver', 'intel_pstate'), scalingAvailableGovernors: value('governors', ['powersave', 'performance']),
                scalingGovernor: value('governor' + i, 'powersave'), energyPerformancePreference: value('epp' + i, 'power'),
                energyPerformanceAvailablePreferences: value('epps', ['power', 'performance']),
                scalingAvailableFrequencies: value('freqs', undefined, false) }));
            this.online = value('online', [0, 1]); this.boost = value('boost', false, false);
            this.intelPstate = { noTurbo: value('noTurbo', false) };
        }
        getAvailableLogicalCores() { reads.push('enumerate'); return this.cores; }
        useCores(v) { writes.push(['cores', v]); if (throwWrite) throw undefined; }
        setGovernorScalingMinFrequency(v) { writes.push(['min', v]); }
        setGovernorScalingMaxFrequency(v) { writes.push(['max', v]); }
        setGovernor(v) { writes.push(['governor', v]); this.cores.forEach(c => { c.scalingGovernor.value = v; }); }
        setEnergyPerformancePreference(v) { writes.push(['epp', v]); if (!ignoreEpp) this.cores.forEach(c => { c.energyPerformancePreference.value = v === 'default' ? 'power' : v; }); }
    }
    const TUXEDODevice = Object.fromEntries(['SIRIUS1602', 'STELLSL15A06', 'GEMINI17I04', 'IBPG10INTEL', 'IBM15I10'].map((s, i) => [s, i + 1]));
    const { CpuWorker } = load('CpuWorker', { '../../common/classes/CpuController': { CpuController },
        '../../common/classes/LogicalCpuController': { ScalingDriver: { intel_pstate: 'intel_pstate', amd_pstate_epp: 'amd_pstate_epp', acpi_cpufreq: 'acpi_cpufreq' } },
        '../../common/models/DefaultProfiles': { TUXEDODevice }, '../../common/models/TccProfile': { FrequencyConfig: { ReducedFrequency: -1 } },
        './DaemonWorker': { DaemonWorker } }, workerSource);
    const profile = { id: 'office', cpu: { energyPerformancePreference: 'power', noTurbo: false } };
    const daemon = { settings, identifyDevice: () => 0, cpuControlOwnership: x.owner, getCurrentProfile: () => profile, logLine: s => logs.push(s) };
    const w = new CpuWorker(daemon); w.updateProfile(profile);
    return { w, writes, reads, profile, daemon, set throwWrite(v) { throwWrite = v; },
        set throwRead(v) { throwRead = v; }, set ignoreEpp(v) { ignoreEpp = v; } };
}

async function run() {
    if (group === 'roundtrip') {
        const x = make(), original = plain(x.state);
        assert.equal(x.owner.state().owner, 'tcc'); assert(x.owner.state().ready);
        const release = set(x, 'system'); assert.equal(release.submitted, true); assert(release.persistence_confirmed);
        assert.equal(release.cpu_replay.attempted, false); assert.equal(x.replays, 0); assert(!x.owner.tccMayWrite());
        assert.equal(fs.readFileSync(x.file, 'utf8'), record(1, 'system'));
        assert.equal(fs.statSync(x.file).mode & 0o777, 0o600);
        const before = x.store.read().fingerprint, noop = set(x, 'system');
        assert(noop.noop); assert.equal(noop.submitted, false); assert.equal(before, x.store.read().fingerprint);
        const restore = set(x, 'tcc'); assert(restore.persistence_confirmed); assert(restore.cpu_replay.validation_passed);
        assert.equal(x.replays, 1); assert(x.owner.tccMayWrite()); assert.equal(restore.state.revision, 2);
        assert.deepEqual(plain(x.state), original, 'No fan/lighting/charging/global settings mutation');
    } else if (['boundaries', 'exit', 'direct'].includes(group)) {
        const x = make(), y = worker(x); set(x, 'system');
        if (group === 'boundaries') { await y.w.start(); await y.w.work(); y.w.updateProfile({ id: 'new', cpu: {} }); await y.w.work(); }
        if (group === 'exit') await y.w.exit();
        if (group === 'direct') { y.w.applyCpuProfile(y.profile); y.w.setCpuDefaultConfig(); }
        assert.equal(y.writes.length, 0, 'external owner must block every CPU write path');
        assert.equal(y.reads.length, 0, 'external owner skips CPU verification as well');
        const original = x.owner; y.daemon.cpuControlOwnership = undefined; await y.w.start(); await y.w.exit();
        assert.equal(y.writes.length, 0, 'missing ownership authority fails closed'); y.daemon.cpuControlOwnership = original;
    } else if (group === 'fence') {
        const x = make(), y = worker(x);
        const starting = y.w.start(); assert(y.writes.length > 0, 'actual synchronous CPU writes executed');
        const queued = Promise.resolve().then(() => y.w.work());
        const released = set(x, 'system'); assert(released.persistence_confirmed);
        const atAck = y.writes.length; await starting; await queued; await y.w.exit();
        assert.equal(y.writes.length, atAck, 'No CPU write crosses the release acknowledgement');
    } else if (group === 'stale') {
        const x = make(), token = x.owner.state().token;
        x.state.profile.cpu.energyPerformancePreference = 'performance'; assert.equal(x.owner.setOwner('system', token).submitted, false);
        const epochToken = x.owner.state().token; x.state.configurationEpoch++;
        assert.equal(x.owner.setOwner('system', epochToken).submitted, false);
        const diskToken = x.owner.state().token; x.state.settingsDiskEpoch = 'disk-epoch-1';
        assert.equal(x.owner.setOwner('system', diskToken).submitted, false);
        const token2 = x.owner.state().token; x.state.settingsMemory.extra = true;
        assert(!x.owner.state().ready); assert.equal(x.owner.setOwner('system', token2).submitted, false);
        x.state.settingsDisk.extra = true; const token3 = x.owner.state().token;
        x.store.replace(x.store.read().fingerprint, { schema_version: 1, revision: 3, owner: 'tcc' });
        assert.equal(x.owner.setOwner('system', token3).submitted, false); assert.equal(x.owner.state().revision, 3);
        assert.equal(x.replays, 0);
        for (const v of ['', 'x', null, 'a'.repeat(65)]) assert.equal(x.owner.setOwner('system', v).submitted, false);
        assert.equal(x.owner.setOwner('SYSTEM', x.owner.state().token).submitted, false);
    } else if (group === 'restart') {
        const x = make(); set(x, 'system'); const oldToken = x.owner.state().token;
        const next = new CpuControlOwnership(() => x.state, () => { throw Error('unexpected replay'); }, x.store);
        assert.equal(next.state().owner, 'system'); assert(!next.tccMayWrite());
        assert.equal(next.setOwner('tcc', oldToken).submitted, false, 'restart invalidates old requests');
        fs.writeFileSync(x.file, record(2, 'tcc')); assert(next.tccMayWrite(), 'next boundary observes external root edit');
        fs.writeFileSync(x.file, 'corrupt'); assert(!next.tccMayWrite()); assert(!next.state().ready);
    } else if (group === 'schema') {
        for (const text of ['null', '{}', record(0, 'system'), record(1, 'SYSTEM'), record(1.5, 'system'),
            record(Number.MAX_SAFE_INTEGER + 1, 'system'), '{"schema_version":1,"revision":1,"owner":"system","owner":"system"}\n',
            '{"schema_version":1,"revision":1,"owner":"system","extra":true}\n', record(1, 'system') + '\n', 'x'.repeat(4097)]) {
            const x = make(); seed(x, text); assert(!x.owner.state().ready, text.slice(0, 100)); assert(!x.owner.tccMayWrite());
            assert.equal(x.owner.setOwner('tcc', 'a'.repeat(64)).submitted, false); assert.equal(fs.readFileSync(x.file, 'utf8'), text);
        }
        const x = make(); seed(x, record(Number.MAX_SAFE_INTEGER, 'tcc'));
        assert.equal(set(x, 'system').submitted, false, 'revision exhaustion cannot wrap');
    } else if (group === 'files') {
        for (const fault of ['mode', 'link', 'symlink', 'directory', 'parent']) {
            const x = make(); seed(x, record(1, 'system'));
            if (fault === 'mode') fs.chmodSync(x.file, 0o644);
            if (fault === 'link') fs.linkSync(x.file, x.file + '.link');
            if (fault === 'symlink') { fs.renameSync(x.file, x.file + '.original'); fs.symlinkSync(x.file + '.original', x.file); }
            if (fault === 'directory') fs.chmodSync(x.directory, 0o770);
            if (fault === 'parent') fs.chmodSync(x.parent, 0o777);
            assert(!x.owner.state().ready, fault); assert(!x.owner.tccMayWrite(), fault);
        }
        const x = make(); seed(x, record(1, 'system'));
        const wrongUid = new CpuOwnerStore(x.directory, process.getuid() + 1, process.getgid());
        assert.throws(() => wrongUid.read(), /not protected/);
    } else if (group === 'failures') {
        for (const fault of ['mkdir', 'temp-open', 'write', 'file-sync', 'directory-sync', 'rename-before', 'rename-after', 'readback', 'cleanup', 'external', 'temp-swap']) {
            let x, renamed = false, temporary, injected = false, saved;
            const fail = () => { throw Object.assign(Error(fault + ' original'), { code: 'EIO' }); };
            const io = new Proxy(fs, { get(target, key) {
                if (key === 'mkdirSync') return (...a) => { if (fault === 'mkdir') fail(); return fs.mkdirSync(...a); };
                if (key === 'openSync') return (...a) => { if (String(a[0]).includes('.cpu-owner-')) { temporary = String(a[0]); if (fault === 'temp-open') fail(); } return fs.openSync(...a); };
                if (key === 'writeFileSync') return (...a) => {
                    if (fault === 'write' || fault === 'cleanup') fail();
                    if (fault === 'temp-swap') { saved = temporary + '.saved'; fs.renameSync(temporary, saved); fs.writeFileSync(temporary, 'foreign', { mode: 0o600 }); }
                    return fs.writeFileSync(...a);
                };
                if (key === 'fsyncSync') return fd => {
                    if (fault === 'file-sync' && fs.fstatSync(fd).isFile()) fail();
                    if (fault === 'directory-sync' && renamed) fail();
                    if (fault === 'external' && !injected && fs.fstatSync(fd).isFile()) { injected = true; fs.writeFileSync(x.file, record(9, 'tcc'), { mode: 0o600 }); }
                    return fs.fsyncSync(fd);
                };
                if (key === 'renameSync') return (...a) => { if (fault === 'rename-before') fail(); const out = fs.renameSync(...a); renamed = true; if (fault === 'rename-after') fail(); return out; };
                if (key === 'readSync') return (...a) => { if (fault === 'readback' && renamed) fail(); return fs.readSync(...a); };
                if (key === 'unlinkSync') return (...a) => { if (fault === 'cleanup') throw Error('cleanup secondary'); return fs.unlinkSync(...a); };
                return target[key];
            } });
            x = make(io); const result = set(x, 'system');
            assert(!result.persistence_confirmed, fault); assert.notEqual(result.error, null, fault); assert.equal(x.replays, 0, fault);
            const submitted = ['rename-before', 'rename-after'].includes(fault) ? null : ['directory-sync', 'readback'].includes(fault) ? true : false;
            assert.equal(result.submitted, submitted, fault);
            if (['directory-sync', 'rename-after', 'readback'].includes(fault)) assert.equal(new CpuOwnerStore(x.directory, process.getuid(), process.getgid()).read().record.owner, 'system');
            if (fault === 'external') assert.equal(fs.readFileSync(x.file, 'utf8'), record(9, 'tcc'));
            if (fault === 'cleanup') { assert.match(result.error, /cleanup original/); assert.match(result.error, /cleanup secondary/); assert(fs.readdirSync(x.directory).some(n => n.startsWith('.cpu-owner-'))); }
            if (fault === 'temp-swap') { const names = fs.readdirSync(x.directory); assert(names.some(n => n.endsWith('.saved'))); assert(names.some(n => fs.readFileSync(path.join(x.directory, n), 'utf8') === 'foreign')); assert.match(result.error, /retained/); }
        }
    } else if (group === 'empty-cpu') {
        const x = make(), y = worker(x);
        y.w.cpuCtrl.cores = []; y.w.cpuCtrl.intelPstate.noTurbo.isAvailable = () => false;
        y.profile.cpu.noTurbo = undefined; x.replay = () => y.w.reapplyAfterOwnershipChange();
        set(x, 'system'); const restored = set(x, 'tcc');
        assert(restored.persistence_confirmed);
        assert(!restored.cpu_replay.validation_passed, 'empty CPU inventory must not produce a successful replay readback');
        assert.equal(y.writes.length, 0, 'no CPU write without a usable inventory');
    } else if (group === 'replay') {
        const x = make(), y = worker(x); x.replay = () => y.w.reapplyAfterOwnershipChange();
        set(x, 'system'); const result = set(x, 'tcc'); assert(result.cpu_replay.validation_passed); assert(y.writes.length > 0);
        set(x, 'system'); y.throwWrite = true; const failed = set(x, 'tcc');
        assert(failed.persistence_confirmed); assert(!failed.cpu_replay.validation_passed); assert.match(failed.error, /undefined/);
        y.throwWrite = false; set(x, 'system'); y.throwRead = true; const read = set(x, 'tcc');
        assert(read.persistence_confirmed); assert.match(read.error, /readback failed/); y.throwRead = false;
        set(x, 'system'); x.state.settingsMemory.cpuSettingsEnabled = false; x.state.settingsDisk.cpuSettingsEnabled = false;
        const before = y.writes.length, disabled = set(x, 'tcc'); assert(disabled.persistence_confirmed);
        assert(!disabled.cpu_replay.attempted); assert.match(disabled.error, /disabled/); await y.w.exit(); assert.equal(y.writes.length, before);
        assert.equal(x.state.settingsMemory.cpuSettingsEnabled, false);
    } else if (group === 'dbus') {
        let members;
        class Interface { static configureMembers(v) { members = v; } }
        class DBusError extends Error { constructor(type, message) { super(message); this.type = type; } }
        const subject = class { constructor(v) { this.value = v; } };
        const { TccDBusInterface, TccDBusData, TccDBusOptions } = load('TccDBusInterface', {
            'dbus-next': { interface: { Interface }, DBusError }, rxjs: { BehaviorSubject: subject },
            '../../common/classes/FnLockController': { FnLockController: class {} },
        });
        const x = make(), y = worker(x); x.replay = () => y.w.reapplyAfterOwnershipChange(); let resets = 0;
        const daemon = { cpuControlOwnership: x.owner, triggerStateCheck() { resets++; }, getChargingWorker: () => ({}) };
        let exported;
        const bus = { requestName: async () => 1, export: (_p, iface) => { exported = iface; }, unexport() {} };
        const { TccDBusService } = load('TccDBusService', {
            'dbus-next': { systemBus: () => bus }, '../../native-lib/TuxedoIOAPI': { TuxedoIOAPI: {} },
            './DaemonWorker': { DaemonWorker }, './TccDBusInterface': { TccDBusInterface, TccDBusData, TccDBusOptions },
        });
        const service = new TccDBusService(daemon, new TccDBusData()); await service.onStart(); await Promise.resolve();
        assert(exported); assert.equal(members.methods.SetCpuControlOwner.inSignature, 'ss');
        const initial = JSON.parse(exported.GetCpuControlStateJSON()); assert(initial.ready); assert.equal(y.writes.length, 0);
        const release = JSON.parse(exported.SetCpuControlOwner('system', initial.token)); assert(release.persistence_confirmed);
        await y.w.start(); await y.w.work(); await y.w.exit(); assert.equal(y.writes.length, 0);
        const restore = JSON.parse(exported.SetCpuControlOwner('tcc', JSON.parse(exported.GetCpuControlStateJSON()).token));
        assert(restore.cpu_replay.validation_passed); assert.equal(resets, 0, 'No global state-check/reset or other worker invocation');
        const missing = new TccDBusInterface(new TccDBusData()); assert.throws(() => missing.SetCpuControlOwner('system', initial.token), /unavailable/);
        await service.onExit();
    } else if (group === 'startup') {
        // Execute the actual main method with fake process/worker constructors; no daemon constructor or native access.
        const x = make(), y = worker(x); set(x, 'system');
        const filename = path.join(source, 'src/service-app/classes/TuxedoControlCenterDaemon.ts');
        const text = fs.readFileSync(filename, 'utf8');
        sourceHashes[filename] = crypto.createHash('sha256').update(text).digest('hex');
        const ast = ts.createSourceFile(filename, text, ts.ScriptTarget.Latest, true);
        const declaration = ast.statements.find(n => ts.isClassDeclaration(n) && n.name.text === 'TuxedoControlCenterDaemon');
        const main = declaration.members.find(n => n.name?.getText(ast) === 'main').getText(ast);
        const generated = ts.transpileModule('export class StartupHarness { ' + main + ' }', {
            compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
        let otherCalls = 0; const timers = [];
        const Other = class { constructor() { this.timeout = 1000; } async work() { otherCalls++; } reapplyProfile() { otherCalls++; } };
        const Owner = class extends CpuControlOwnership { constructor(context, replay) { super(context, replay, x.store); } };
        const names = ['DisplayRefreshRateWorker', 'StateSwitcherWorker', 'ChargingWorker', 'DisplayBacklightWorker',
            'WebcamWorker', 'FanControlWorker', 'YCbCr420WorkaroundWorker', 'GpuInfoWorker', 'CpuPowerWorker', 'PrimeWorker',
            'TccDBusService', 'ODMProfileWorker', 'ODMPowerLimitWorker', 'KeyboardBacklightListener', 'NVIDIAPowerCTRLListener', 'AvailabilityService'];
        const fakeImports = Object.fromEntries(names.map(n => [n, Other]));
        const m = { exports: {} };
        vm.runInNewContext('(function(exports){' + generated + '\n})', {
            ...fakeImports, CpuControlOwnership: Owner, CpuWorker: y.w.constructor,
            process: { argv: ['tccd', '--start'], geteuid: () => 0, exit: () => { throw Error('unexpected exit'); } },
            tccPackage: { version: '3.0.10' }, os: { arch: () => 'x64' }, fs, console,
            setInterval: fn => { timers.push(fn); return timers.length; },
        }, { timeout: 1000 })(m.exports);
        const settingsPath = path.join(x.parent, 'settings.json');
        fs.writeFileSync(settingsPath, JSON.stringify(x.state.settingsMemory), { mode: 0o600 });
        const h = Object.create(m.exports.StartupHarness.prototype);
        Object.assign(h, { workers: [], listeners: [], dbusData: {}, cpuOwnershipConfigEpoch: 1, settings: x.state.settingsMemory,
            config: { pathSettings: settingsPath, readSettings: () => JSON.parse(fs.readFileSync(settingsPath, 'utf8')) },
            getCurrentProfile: () => y.profile, identifyDevice: () => 0, loadConfigsAndProfiles() {},
            handleArgumentProgramFlow: async () => {}, setupSignalHandling: async () => {}, logLine: s => logs.push(s),
            async startWorkers() { this.cpuWorker.updateProfile(y.profile); await this.cpuWorker.start(); return true; },
        });
        await h.main(); assert.equal(h.started, true); assert(h.cpuControlOwnership instanceof CpuControlOwnership);
        assert.equal(y.writes.length, 0, 'persisted external owner is honored on actual daemon startup');
        assert(timers.length > 1); const state = h.cpuControlOwnership.state(); assert(state.ready, state.error);
        const oldToken = state.token;
        const replacement = settingsPath + '.replacement'; fs.writeFileSync(replacement, fs.readFileSync(settingsPath), { mode: 0o600 });
        fs.renameSync(replacement, settingsPath);
        assert.notEqual(oldToken, h.cpuControlOwnership.state().token, 'same-value replacement invalidates settings epoch');
        assert.equal(h.cpuControlOwnership.setOwner('tcc', oldToken).submitted, false);
        const restored = h.cpuControlOwnership.setOwner('tcc', h.cpuControlOwnership.state().token);
        assert(restored.persistence_confirmed); assert(restored.cpu_replay.validation_passed);
        assert.equal(otherCalls, 0, 'actual startup-wired handoff never resets or invokes another worker');
    } else if (group === 'policy' || group === 'policy-old') {
        assert.notEqual(process.getuid(), 0, 'unprivileged policy qualification must not run as root');
        assert(modules, 'explicit existing dbus-next input required');
        const dbus = require(modules), childProcess = require('node:child_process');
        const x = make(), xml = v => v.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;');
        const selected = group === 'policy-old' ? workerSource : source;
        const policy = path.join(selected, 'src/dist-data/com.tuxedocomputers.tccd.conf');
        const conf = path.join(x.parent, 'private-bus.conf');
        const socketDirectoryFd = fs.openSync(x.parent, fs.constants.O_RDONLY | fs.constants.O_DIRECTORY);
        const socketPath = '/proc/' + process.pid + '/fd/' + socketDirectoryFd + '/bus.sock';
        fs.writeFileSync(conf, '<busconfig><type>system</type><listen>unix:path=' + socketPath + '</listen>' +
            '<auth>EXTERNAL</auth><policy context="default"><allow user="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy>' +
            '<include>' + xml(policy) + '</include><policy user="' + process.getuid() + '"><allow own="com.tuxedocomputers.tccd"/></policy></busconfig>', { mode: 0o600 });
        const broker = childProcess.spawn('/usr/bin/dbus-daemon', ['--nofork', '--config-file=' + conf, '--print-address=1'], { stdio: ['ignore', 'pipe', 'pipe'] });
        let errors = '', server, client, service;
        broker.stderr.on('data', c => { errors += c; });
        const exited = new Promise(resolve => broker.once('exit', (code, signal) => resolve({ code, signal })));
        const bounded = async p => {
            let timer; try { return await Promise.race([p, new Promise((_r, reject) => { timer = setTimeout(() => reject(Error('private bus timeout: ' + errors)), 5000); })]); }
            finally { clearTimeout(timer); }
        };
        try {
            const address = await bounded(new Promise((resolve, reject) => {
                let text = ''; broker.once('error', reject);
                broker.stdout.on('data', c => { text += c; if (text.includes('\n')) resolve(text.split('\n')[0]); });
                broker.once('exit', () => reject(Error('private broker exited before readiness: ' + errors)));
            }));
            assert(fs.lstatSync(path.join(x.parent, 'bus.sock')).isSocket(), 'private Unix socket stays inside selected scratch root');
            server = dbus.sessionBus({ busAddress: address }); client = dbus.sessionBus({ busAddress: address });
            server.on('error', e => logs.push(String(e))); client.on('error', e => logs.push(String(e)));
            const ifaceClasses = load('TccDBusInterface', { 'dbus-next': dbus, rxjs: { BehaviorSubject: class {} },
                '../../common/classes/FnLockController': { FnLockController: class {} } });
            let setters = 0;
            const daemon = { cpuControlOwnership: { state: () => x.owner.state(), setOwner: (...a) => { setters++; return x.owner.setOwner(...a); } },
                triggerStateCheck() { throw Error('global state-check prohibited'); }, getChargingWorker: () => ({}) };
            const { TccDBusService } = load('TccDBusService', {
                'dbus-next': { ...dbus, systemBus: () => server }, '../../native-lib/TuxedoIOAPI': { TuxedoIOAPI: {} },
                './DaemonWorker': { DaemonWorker }, './TccDBusInterface': ifaceClasses,
            });
            service = new TccDBusService(daemon, new ifaceClasses.TccDBusData()); await service.onStart();
            const busObject = await bounded(client.getProxyObject('org.freedesktop.DBus', '/org/freedesktop/DBus'));
            const unique = await bounded(busObject.getInterface('org.freedesktop.DBus').GetNameOwner('com.tuxedocomputers.tccd'));
            const object = await bounded(client.getProxyObject('com.tuxedocomputers.tccd', '/com/tuxedocomputers/tccd'));
            const initial = JSON.parse(await bounded(object.getInterface('com.tuxedocomputers.tccd').GetCpuControlStateJSON()));
            assert(initial.ready); assert.equal(initial.owner, 'tcc', 'read-only state method remains available');
            for (const destination of ['com.tuxedocomputers.tccd', unique]) {
                for (const interfaceName of ['com.tuxedocomputers.tccd', undefined, 'org.example.Bypass']) {
                    const message = new dbus.Message({ destination, path: '/com/tuxedocomputers/tccd', interface: interfaceName,
                        member: 'SetCpuControlOwner', signature: 'ss', body: ['system', initial.token] });
                    await assert.rejects(bounded(client.call(message)), e => e.type === 'org.freedesktop.DBus.Error.AccessDenied',
                        'unprivileged CPU setter must be denied for named/unique destination and every interface');
                }
            }
            assert.equal(setters, 0); assert.equal(x.store.read().record.owner, 'tcc');
        } finally {
            if (service) await service.onExit();
            if (server) server.disconnect(); if (client) client.disconnect();
            broker.kill('SIGTERM'); const done = await bounded(exited);
            fs.closeSync(socketDirectoryFd);
            assert(done.code === 0 || done.signal === 'SIGTERM', 'private broker terminated and reaped: ' + JSON.stringify(done) + ' ' + errors);
        }
    } else {
        throw Error('Unknown test group: ' + group);
    }
    console.log(JSON.stringify({ group, source_hashes: sourceHashes, actual_classes: true, hardware_io: false, passed: true }));
}
run().catch(error => { console.error(error); process.exitCode = 1; });
