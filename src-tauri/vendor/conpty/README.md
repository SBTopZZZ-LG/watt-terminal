# Vendored ConPTY host (Windows)

Watt ships Microsoft's ConPTY host next to `watt-terminal.exe` on Windows so
full-screen TUIs (for example OpenCode) keep the shell alive when they exit.

`portable-pty` (0.9.0) loads `conpty.dll` by bare name, so the exe directory is
searched first. If the DLL is missing it silently falls back to the inbox
`conhost`, and the shell dies with the TUI. Both files must sit in the same
directory as the exe. `conpty.dll` spawns `OpenConsole.exe` from its own
directory.

## Contents

| File | Path | SHA-256 | Size |
| --- | --- | --- | --- |
| `conpty.dll` | `x64/conpty.dll` | `e971856f89748dc3b458125e9afdd79b5e9574a7c937676116d3cbbd3b71f492` | 119,608 bytes |
| `OpenConsole.exe` | `x64/OpenConsole.exe` | `b17e48b07ef153b420f0a6e178bacd31fb4c3390e21ae746586213fc0719a775` | 1,089,336 bytes |

- Package: `Microsoft.Windows.Console.ConPTY` version `1.25.261109001`
- Source: https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY/1.25.261109001
- Project: https://github.com/microsoft/terminal
- License: MIT (see `LICENSE-MIT.txt`). The nupkg has no license file; the text is
  taken from the `LICENSE` file in microsoft/terminal.
- Signer: Authenticode, `CN=Microsoft Corporation`, status Valid.

## Updating

1. Download the new package from nuget.org (`.nupkg`, a zip) and extract it to an
   empty directory.
2. Replace `x64/conpty.dll` with `runtimes/win-x64/native/conpty.dll` and
   `x64/OpenConsole.exe` with `build/native/runtimes/x64/OpenConsole.exe`.
3. Check Authenticode status is Valid and signer is Microsoft Corporation.
4. Update the SHA-256 values and sizes above.
5. Rerun the Windows tests (`cargo test --test windows_session`).
