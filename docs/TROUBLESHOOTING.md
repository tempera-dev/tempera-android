# Troubleshooting

## `sdkmanager` cannot find Java

Run:

```bash
brew install --cask temurin@21
export JAVA_HOME="$(/usr/libexec/java_home -v 21)"
```

Set `ANDROID_SDK_ROOT` to the official command-line-tools installation and rerun `tempera-android doctor`.

## A requested system image is unavailable

Use a supported profile and API explicitly:

```bash
tempera-android install --profile google --api 36
tempera-android device create --name tempera-google --profile google --api 36
```

The product invokes `sdkmanager`, `avdmanager`, `emulator`, and `adb` directly; it does not depend on the newer `android` CLI.

## The emulator boots but the internet does not work

```bash
tempera-android --serial emulator-5554 network --json
```

Restart the managed emulator with a cold boot:

```bash
tempera-android --serial emulator-5554 device stop
tempera-android device start tempera-google --cold --headless
```

A corporate firewall, VPN, proxy, or DNS filter on the Mac can also affect the
emulator because outbound traffic uses the host network.

## Access a server running on the Mac

Inside Android, `127.0.0.1` means the emulator itself. Use `10.0.2.2` to reach a
service bound to the Mac's loopback interface. For example, Android can reach a
Mac server on port 8000 at `http://10.0.2.2:8000`.

## An APK fails with `INSTALL_FAILED_NO_MATCHING_ABIS`

Install an APK containing a native ABI matching the target image. Apple Silicon
managed emulators use `arm64-v8a`; typical Linux CI emulators use `x86_64`.

## A banking, DRM, or game app refuses to run

The Play profile is production-signed and includes Play Services, but it remains
an emulator and does not become physical hardware. Apps may require hardware
attestation, Widevine levels, telephony, NFC secure elements, or vendor-specific
components unavailable in an AVD. This project does not bypass those checks.

## Resetting a managed emulator removed all apps

That is expected. Reset is destructive and only applies to a named,
Tempera-managed AVD. Use a separate managed device when preserving the original
environment matters:

```bash
tempera-android device create --name tempera-google-2 --profile google --api 36
tempera-android device list --json
```
