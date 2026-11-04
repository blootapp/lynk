; Custom NSIS hooks for Lynk.
;
; After an update replaces lynk.exe, Windows keeps showing the old icon on the
; taskbar: the button takes it from the Start menu shortcut (same app id), and
; that icon comes from the shell's icon cache. Tauri's installer only sends
; SHCNE_ASSOCCHANGED, which doesn't refresh it; ie4uinit -show does.
;
; The installer is 32-bit, so $SYSDIR is SysWOW64 on 64-bit Windows, and
; ie4uinit.exe lives only in the real System32 (Sysnative from here).

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  StrCpy $0 "$WINDIR\Sysnative\ie4uinit.exe"
  IfFileExists $0 +2
    StrCpy $0 "$SYSDIR\ie4uinit.exe"
  DetailPrint "Refreshing the Windows icon cache"
  nsExec::Exec '"$0" -show'
  Pop $0 ; exit code
  Pop $0
!macroend
