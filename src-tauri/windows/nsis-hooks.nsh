; Custom NSIS hooks for Lynk.
;
; After an update replaces lynk.exe, the shortcuts (desktop, Start, pinned)
; can keep showing its old icon from the shell's icon cache. Tauri's installer
; only sends SHCNE_ASSOCCHANGED, which doesn't refresh it; ie4uinit -show does.
; (The running window's taskbar button is set by the app itself, lib.rs.)
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
