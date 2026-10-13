# Experimental Logitech C54D receiver support

This branch adds Logitech USB VID **046D**, PID **C54D** to better-logihub's HID++ receiver discovery list. The receiver was observed with a **PRO X SUPERLIGHT 2**, previously showing as `direct device / USB Receiver / unreachable: HID++ 1.0 error 1 (invalid sub-id)`.

**Important:** C54D discovery and live DPI setting have been verified on the local Superlight 2. This branch has not been integrated with CafeLauncher. Onboard profile memory was not written.

## Download the test executable

1. Open the [experimental Windows build workflow](../../actions/workflows/c54d-windows.yml).
2. Open a successful run on `feature/c54d-superlight2-receiver`.
3. Download the `logihub-c54d-test-windows-x64` artifact (GitHub sign-in may be required).
4. Extract `logihub.exe` **to a separate directory**. Do not replace CafeLauncher's existing cached executable.

## Safe test sequence

With the Superlight 2 turned on and G HUB closed, run in PowerShell from the extracted directory:

```powershell
$expected = (Get-Content .\logihub.exe.sha256 -Raw).Trim()
$actual = (Get-FileHash .\logihub.exe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "SHA-256 mismatch" }
.\logihub.exe --json list
```

The output should show `type: wireless device`, `model_id: pro_x_2_wireless_mouse`, and `status: connected` if device discovery succeeds. Note its `index` value. Then read DPI without modifying it:

```powershell
.\logihub.exe dpi get --device <INDEX>
```

Replace `<INDEX>` with the actual number from the list; do **not** copy angle brackets.

The build artifact is a temporary test build, not an official upstream release. The original MIT license remains in the fork.

## Extended DPI debug output (read-only)

Use the newest successful Windows workflow artifact. Run:

```powershell
.\logihub.exe --json list
.\logihub.exe dpi get --device 1
.\logihub.exe onboard mode get --device 1
.\logihub.exe dpi diagnose --device 1 --target 1800 --verbose
```

Confirm the mouse is still at index 1 first. `dpi diagnose` performs only feature discovery and **read** calls. Verbose output shows firmware version (without unit ID or serial), 0x8100 mode, 0x2202 sensor axes and DPI ranges, raw request templates and response parameters, and the *prospective* setter payload. It does **not** transmit that payload.

## Current finding (13 October 2026)

On the local PRO X 2, firmware `MPM32.05` build `0029`, the read-only mode query returned **onboard mode** (`0x01`). The 0x2202 sensor lists both X and Y axes, LOD, and 1800 DPI is inside the advertised `1010-2000 step 10` range. The existing function-6 payload `00 07 08 07 08 02` matches the layout in [libratbag's Superlight 2 implementation](https://github.com/libratbag/libratbag/pull/1896) and [Solaar's extended DPI setting](https://github.com/pwr-Solaar/Solaar/blob/master/lib/logitech_receiver/settings_templates.py). The old error message was wrong: HID++ 2.0 code `5` means **Logitech internal error**, per the [HID++ specification](https://lekensteyn.nl/files/logitech/logitech_hidpp_2.0_specification_draft_2012-06-04.pdf), rather than invalid argument.

**Verified on hardware:** the same 0x2202 function-6 write succeeds in host mode, and a separate `dpi get` returns 1800. Restoring onboard mode returns the reported DPI to 800. This isolates the failed live write to the operating mode rather than DPI value, feature index, or payload layout. [Solaar notes that onboard profiles may need to be disabled](https://github.com/pwr-Solaar/Solaar/blob/master/lib/logitech_receiver/settings_templates.py); [libratbag routes devices with onboard profiles through profile handling](https://github.com/libratbag/libratbag/pull/1896).

## Live DPI command

The owner approved the host-mode switch and DPI write on 13 October 2026. For this model, `dpi set` now switches from onboard to host mode when necessary, sends the existing 0x2202 payload once, and verifies the read-back. It **leaves host mode active** on success so a separate `dpi get` returns the requested value. If the write or verification fails after the switch, it restores onboard mode. Other mouse models keep their existing setter path. Host mode can change button behavior while G HUB is closed; keep a second pointing device available during the first deployment.

```powershell
.\logihub.exe dpi set 1800 --device 1
.\logihub.exe dpi get --device 1
.\logihub.exe onboard mode get --device 1
```

To return to the previous onboard profile and DPI explicitly:

```powershell
.\logihub.exe onboard mode onboard --device 1
.\logihub.exe onboard mode get --device 1
```

The mode-only command accepts this model's profile format `0x06`/`0x07` without trying to parse or write its profile sectors. DPI after power cycling, receiver reconnection, or deployment on other firmware has not yet been measured.
