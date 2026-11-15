; NSIS hooks for the Windows installer (wired up as bundle.windows.nsis.installerHooks).
; Tauri's template already writes the Installed apps entry and the Start Menu and Desktop
; shortcuts, and removes them on uninstall. Its per-user install-location key
; (HKCU\Software\watt\Watt) is left behind by the uninstaller, so remove it here.

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\watt\Watt"
  DeleteRegKey /ifempty HKCU "Software\watt"
!macroend
