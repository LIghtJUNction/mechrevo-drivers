# TCC fan I/O error contract

This local Arch recipe rebuilds the C++ API/NAPI module, TypeScript service, packaged daemon and Electron app from TCC 3.0.10 GPL source. It does not patch only app.asar.

Source archive SHA256: `d192391c559789c8f62af0e614e37e1222c9aa9753c61a30e3149b35d1969532` (upstream tag v3.0.10). Use Node **24.22.0**, npm **11.19.1**, and system Electron **42.11.11** at `/usr/lib/electron42`. The unchanged npm lock SHA256 is `e52393cc472aa58d376e204867e9fb7a42a14ca0e588be69c4509720229a56a1`. The locked pkg tool downloads its separate Node **24.18.0** packaging runtime. Keep Node and npm isolated from the global installation; set private npm/pkg/Electron/node-gyp caches. `npm ci` verifies locked registry integrity; Git dependencies are pinned by lockfile commit and do not have registry SRI.

Run `makepkg` from this directory with the pinned Node in PATH, or set `TCC_CONTRACT_ROOT` to this reviewed repository when copying the recipe elsewhere. The recipe requires dependencies already installed; it has no dependency-install command. Its check step runs the four contract tests explicitly. For a separate test invocation, set:

```sh
export TCC_NODE=/path/to/node-v24.22.0-linux-x64/bin/node
export TCC_NODE_INCLUDE=/path/to/node-v24.22.0-linux-x64/include/node
export TCC_NODE_ADDON_API=/path/to/patched-source/node_modules/node-addon-api
export TCC_TYPESCRIPT_MODULE=/path/to/patched-source/node_modules/typescript
export MECHREVO_TEST_TMPDIR=/path/to/private/test-tmp
python -B -m unittest discover -s tests -p test_tcc_fan_error_contract.py -v
```

The GPL fixtures retain upstream notices and file hashes in `tests/fixtures/tcc_3_0_10/provenance.json`. The explicit validation ran **4 tests, no skips**, with the original source and old-worker negative controls rejected. In an ordinary suite invocation, native/TypeScript tests can skip unless those environment variables are provided; a green default suite alone does not establish this contract.

A failed Uniwill speed read now returns before arithmetic or output mutation, avoiding an uninitialized read. NAPI publishes output only on success. The service returns its existing -1 sentinel on failed reads and rejects failed writes while preserving the successful Promise<void> interface. Fan operations settle together; failed writes retain retryable targets and still publish actual telemetry. The worker serializes work and catches failures so the next dispatch retries. Tests use actual source with mocked I/O; they never open hardware. Init/exit mode boolean results remain a separate future audit.

Validation covers a full production source build, extracted app.asar service behavior and byte-identical packaged native modules, including the statically extracted daemon native module. The daemon contains V8 bytecode: build provenance and static payload identity do not establish running-daemon behavior. The package produced for review has **not been installed**, and its daemon has **not been started**. No `.install` script or service enable/start hook is supplied; packaging does not restart the existing installation.
