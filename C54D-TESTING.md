# Experimental Logitech C54D receiver support

This branch adds Logitech USB VID **046D**, PID **C54D** to better-logihub's HID++ receiver discovery list. The receiver was observed with a **PRO X SUPERLIGHT 2**, previously showing as `direct device / USB Receiver / unreachable: HID++ 1.0 error 1 (invalid sub-id)`.

**Important:** C54D discovery and read-only DPI diagnostics have now been verified on the local Superlight 2. Live DPI setting in host mode has **not** been verified. Do not overwrite CafeLauncher's helper or enable onboard profile writes based on this change alone.

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

Replace `<INDEX>` with the actual number from the list; do **not** copy angle brackets. Report both command outputs to validate support before integrating the new binary into CafeLauncher.

The build artifact is a temporary test build, not an official upstream release. The original MIT license remains in the fork.

## Extended DPI debug output (read-only)

Use the newest successful Windows workflow artifact. Run:

```powershell
.\logihub.exe --json list
.\logihub.exe dpi get --device 1
.\logihub.exe onboard mode get --device 1
.\logihub.exe dpi diagnose --device 1 --target 1800 --verbose
```

Confirm the mouse is still at index 1 first. `dpi diagnose` performs only feature discovery and **read** calls. Verbose output shows firmware version (without unit ID or serial), 0x8100 mode, 0x2202 sensor axes and DPI ranges, raw request templates and response parameters, and the *prospective* setter payload. It does **not** transmit that payload. Do not repeatedly retry `dpi set` while the firmware rejects it.

## Current finding (13 October 2026)

On the local PRO X 2, firmware `MPM32.05` build `0029`, the read-only mode query returned **onboard mode** (`0x01`). The 0x2202 sensor lists both X and Y axes, LOD, and 1800 DPI is inside the advertised `1010-2000 step 10` range. The existing function-6 payload `00 07 08 07 08 02` matches the layout in [libratbag's Superlight 2 implementation](https://github.com/libratbag/libratbag/pull/1896) and [Solaar's extended DPI setting](https://github.com/pwr-Solaar/Solaar/blob/master/lib/logitech_receiver/settings_templates.py). The old error message was wrong: HID++ 2.0 code `5` means **Logitech internal error**, per the [HID++ specification](https://lekensteyn.nl/files/logitech/logitech_hidpp_2.0_specification_draft_2012-06-04.pdf), rather than invalid argument.

**High confidence:** receiver addressing, feature index, getter offsets, axis payload, and requested DPI range are correct. **Moderate confidence:** onboard mode causes the rejected live setter; [Solaar notes that onboard profiles may need to be disabled](https://github.com/pwr-Solaar/Solaar/blob/master/lib/logitech_receiver/settings_templates.py), while [libratbag routes devices with onboard profiles through profile handling](https://github.com/libratbag/libratbag/pull/1896). The exact cause of this firmware's internal error remains unproven until an approved host-mode write test. The CLI now stops `dpi set` for this model while onboard mode is active, without changing mode or DPI.

## Approval-gated host-mode test

**Do not run this sequence until the owner approves a mode switch and one DPI write.** Host mode can change button behavior while G HUB is closed. Keep a second pointing device available. These commands do not write onboard profile memory, and the final command restores the previous mode:

```powershell
.\logihub.exe onboard mode get --device 1
.\logihub.exe dpi get --device 1
.\logihub.exe onboard mode host --device 1
.\logihub.exe onboard mode get --device 1
.\logihub.exe dpi set 1800 --device 1
.\logihub.exe dpi get --device 1
.\logihub.exe onboard mode onboard --device 1
.\logihub.exe onboard mode get --device 1
```

If the live setter still returns code `5` in host mode, record the error and restore onboard mode. The host-mode path is a separate experiment from onboard-memory editing.
