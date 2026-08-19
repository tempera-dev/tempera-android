# Mobile gauntlet device lane

This child lane turns Tempera Gym / Tempera Evals mobile tasks into bounded Android execution evidence without moving benchmark truth into the device runtime.

## Ownership

- **Tempera Gym** owns deterministic task environments, seeded development worlds, reference trajectories, and semantic calibration. Semantic replay is not real-device capability evidence.
- **Tempera Android** owns device/runtime execution: exact emulator identity, fixture APK closure, network/orientation setup, bounded agent execution, and raw before/run/after artifacts.
- **Tempera Evals** owns the construct, sealed task population, hidden perturbation schedule, model-blind terminal/effect verifier, comparison statistics, and release decision.

The separation is deliberate. Android must not receive hidden required/forbidden effects merely to produce a score, and Gym must not be allowed to relabel semantic replay as Android evidence.

## Run-plan contract

`contracts/mobile-gauntlet-device-plan.schema.json` describes `tempera.android.mobile-gauntlet-plan/v1`. The executable validator in `scripts/mobile-gauntlet-device.py` is stdlib-only and fails closed on contract drift.

V1 requires:

- a disposable `emulator-*` target; physical devices are rejected;
- `networkPolicy=deny`;
- `fixtureClass=disposable-synthetic`;
- exact fixture APK SHA-256 values and relative paths;
- an exact Gym source revision and adapter digest;
- an Evals suite digest and perturbation-design digest;
- one declared fixture start package or a `tempera-fixture://` launch URI;
- clear/reset operations only for packages already declared in the plan;
- `maxSteps <= 40`, matching the compiled `tempera-android run` bound in the parent runtime;
- append-never evidence output.

The run plan contains task text because the local planner needs it. The evidence manifest contains only `taskSha256`; raw task text is not copied into the manifest.

## Execution sequence

The device harness performs the following bounded sequence:

1. validate the exact run plan;
2. require the declared emulator to be online;
3. hash every local fixture APK before installation;
4. record and then disable emulator Wi-Fi/mobile data;
5. freeze the requested orientation;
6. install the exact fixture APK set;
7. clear only explicitly declared fixture packages;
8. launch only the declared package or `tempera-fixture://` URI;
9. capture device info, the compiled Android eval catalog, and a before snapshot;
10. invoke the canonical `tempera-android run` loop with the existing revision-bound and approval-gated action path;
11. capture the raw run response and an independent after snapshot;
12. restore prior orientation and connectivity settings even when execution fails;
13. write a content-addressed evidence manifest over the raw artifacts.

An agent process failure is evidence, not automatically an infrastructure failure. The harness still attempts to capture final Android state. Setup, identity, APK-digest, or evidence-integrity failures are infrastructure failures.

## Evidence and grading

The output directory is append-never and contains raw JSON envelopes plus `evidence.json`. `evidence.json` records:

- plan and task hashes;
- suite/case/seed identity;
- Gym/Evals/perturbation bindings;
- emulator build facts;
- exact fixture APK digests and observed package versions;
- execution transport/orientation/network policy/step budget;
- hashes of each raw Android artifact;
- the agent process exit code.

`officialResultEligible` remains `false`. A sealed Evals importer/verifier must independently validate the output, fixture-effect ledger, hidden postconditions, execution attestation, and release policy before a device result can become official.

## Current frontier task families

The companion Gym/Evals work defines a core five-family gauntlet and a separate extreme lane. The extreme v1 families are:

- cross-app itinerary arithmetic with interruption recovery, exactly-once communication, and privacy constraints;
- access staging with request-vs-security-policy reconciliation, conditional clarification, exact identity/duration selection, restart recovery, and a hard stop before activation;
- procurement drafting with read-only inventory evidence, quote/file reconciliation, numeric constraints, process recreation, artifact selection, and a hard stop before submission.

The Gym extreme adapter is currently bound to source revision `40009bb0c75e01828071b0f492b2ca420abd19de` and adapter digest `c1e7ef3c39d0491a66ff8d0a6f01aa2cee0659d82edf073fbccad24f6a7abaf5`. Those are development bindings, not a statement that the device fixtures have been released.

## What is not claimed yet

This child does not include the sealed fixture APK population, model credentials, hidden effect ledger, or a published model score. It therefore does not claim AndroidWorld/MobileWorld superiority, frontier-model performance, or an official Tempera mobile capability result.

The next release gate is to build/pin the synthetic multi-app fixture APK set, produce real emulator runs for the same conceptual Gym families, import the evidence into Tempera Evals, and require the hidden verifier to agree across repeated seed/perturbation cells.
