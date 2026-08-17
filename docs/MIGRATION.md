# Migration from Android Simulator

The complete Android Simulator history is preserved in this repository and in the `agent/android-computer-use` baseline. The Rust product is `tempera-android`; `android-sim` and `android-agent` are not shipped aliases.

Existing AVDs remain where Android created them. Tempera only records AVDs it creates under `TEMPERA_ANDROID_HOME/devices`; attach an existing emulator or physical target by ADB serial. For a deliberate registry import, run `tempera-android migrate legacy-avd NAME --yes` (optionally `--source PATH`). It copies only `instances/NAME.json` into Tempera metadata, refuses to overwrite a managed record, and never invokes the SDK or moves, resets, or deletes AVD data.

The historical Python implementation remains a behavioral reference during the alpha port. Release artifacts contain the Rust CLI and optional Java companion, not a Python host runtime.
