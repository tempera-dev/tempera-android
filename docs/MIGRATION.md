# Migration from Android Simulator

The complete Android Simulator history is preserved in this repository and in the `agent/android-computer-use` baseline. The Rust product is `tempera-android`; `android-sim` and `android-agent` are not shipped aliases.

Existing AVDs remain where Android created them. Tempera only records AVDs it creates under `TEMPERA_ANDROID_HOME/devices`; attach an existing emulator or physical target by ADB serial. Metadata migration must be an explicit future command and will never move or delete AVD data automatically.

The historical Python implementation remains a behavioral reference during the alpha port. Release artifacts contain the Rust CLI and optional Java companion, not a Python host runtime.
