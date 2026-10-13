# Experimental Logitech C54D receiver support

This branch adds Logitech USB VID **046D**, PID **C54D** to better-logihub's HID++ receiver discovery list. The receiver was observed with a **PRO X SUPERLIGHT 2**, previously showing as `direct device / USB Receiver / unreachable: HID++ 1.0 error 1 (invalid sub-id)`.

**Important:** Recognizing a receiver PID is only the first step. This has not yet been verified against real C54D hardware. Do not overwrite the original helper or enable onboard profile writes based on this change alone.

## Download the test executable

1. Open the [experimental Windows build workflow](../../actions/workflows/c54d-windows.yml).
2. Open a successful run on `feature/c54d-superlight2-receiver`.
3. Download the `logihub-c54d-test-windows-x64` artifact (GitHub sign-in may be required).
4. Extract `logihub.exe` **to a separate directory**. Do not replace CafeLauncher's existing cached executable.

## Safe test sequence

With the Superlight 2 turned on and G HUB closed, run in PowerShell from the extracted directory:

```powershell
Get-FileHash .\logihub.exe -Algorithm SHA256
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
.\logihub.exe dpi diagnose --device 1 --target 1800
```

Confirm the mouse is still at index 1 first. `dpi diagnose` performs only feature discovery and **read** calls. It reports whether HID++ feature 0x2202 or 0x2201 exists, their feature indices, the first 10 bytes of sensor 0's getter reply, and the *prospective* extended setter payload. It explicitly does **not** transmit that payload. Send the full diagnostic output for further investigation, and do not repeatedly retry `dpi set` while the firmware rejects it.
