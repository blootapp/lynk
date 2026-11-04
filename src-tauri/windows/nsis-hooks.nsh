; Custom NSIS hooks for lynk (bundle > windows > nsis > installerHooks).
;
; Why: Windows draws lynk's taskbar button with the icon of the shortcut lynk
; was started from, and a direct start (after an update, from the finish page)
; uses the Start menu shortcut with the same app ID. Tauri's shortcuts say
; "lynk.exe, icon 0", which Explorer serves from its icon cache. That cache is
; keyed by the path and isn't refreshed when lynk.exe gets a new icon: after
; the icon change in 0.1.3 the taskbar kept the old one while the title bar and
; the tray showed the new one. ie4uinit -show and SHChangeNotify don't clear
; that entry (tried on a real PC); an icon path Explorer hasn't seen is read
; fresh.
;
; So the installer keeps a copy of the icon named after a hash of its contents
; ($INSTDIR\lynk-<hash>.ico) and points lynk's shortcuts and its Installed apps
; entry at it. The name only changes when the icon does, so ordinary updates
; leave the shortcuts as they are.
;
; Note: this file is included near the top of Tauri's installer.nsi, before
; PRODUCTNAME, VERSION and the like are defined. Code outside the hook macros
; (the macro below, .onGUIEnd) must not use them.

; Hash the icon at compile time: icons/icon.ico, next to this file's folder.
!define LYNK_ICON_SRC "${__FILEDIR__}\..\icons\icon.ico"
!tempfile LYNK_ICON_TAG_FILE
!system `powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'Stop'; $h = (Get-FileHash -Algorithm SHA256 -LiteralPath '${LYNK_ICON_SRC}').Hash.Substring(0, 12).ToLowerInvariant(); Set-Content -Encoding Ascii -LiteralPath '${LYNK_ICON_TAG_FILE}' -Value ('!define LYNK_ICON_TAG ' + $h)"` = 0
!include "${LYNK_ICON_TAG_FILE}"
!delfile "${LYNK_ICON_TAG_FILE}"
!undef LYNK_ICON_TAG_FILE
!ifndef LYNK_ICON_TAG
  !error "nsis-hooks.nsh: couldn't hash ${LYNK_ICON_SRC}"
!endif

Var LynkExe     ; $INSTDIR\lynk.exe
Var LynkIcon    ; $INSTDIR\lynk-<hash>.ico, set once the copy is in place
Var LynkDesktop ; $DESKTOP\Lynk.lnk

; Points a shortcut at the icon copy if it's one of lynk's (its target is
; $LynkExe). Only the icon changes, so the shortcut keeps its app ID and
; everything else. Uses $0-$3.
!macro LYNK_SET_SHORTCUT_ICON shortcut
  ${If} ${FileExists} "${shortcut}"
    !insertmacro IsShortcutTarget "${shortcut}" "$LynkExe"
    Pop $0
    ${If} $0 = 1
      !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
      ${If} $0 P<> 0
        ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}",.r1)'
        ${If} $1 P<> 0
          ${IPersistFile::Load} $1 '("${shortcut}", ${STGM_READWRITE})'
          ${IShellLink::SetIconLocation} $0 '("$LynkIcon", 0)'
          ${IPersistFile::Save} $1 '("${shortcut}", 1)'
          ${IUnknown::Release} $1 ""
        ${EndIf}
        ${IUnknown::Release} $0 ""
      ${EndIf}
      ; SHCNE_UPDATEITEM, SHCNF_PATHW | SHCNF_FLUSHNOWAIT: redraw it now
      System::Call 'shell32::SHChangeNotify(i 0x2000, i 0x3005, w "${shortcut}", p 0)'
    ${EndIf}
  ${EndIf}
!macroend

; The finish page's "Create desktop shortcut" box makes that shortcut after
; the install section, so POSTINSTALL can't have seen it. This runs once the
; installer window has closed (not at all for /S, where the install section
; makes the shortcut itself).
Function .onGUIEnd
  ${If} $LynkIcon != ""
    !insertmacro LYNK_SET_SHORTCUT_ICON "$LynkDesktop"
  ${EndIf}
FunctionEnd

; After Tauri has copied the files and made (or, for updates, kept) the
; shortcuts.
!macro NSIS_HOOK_POSTINSTALL
  Push $0
  Push $1
  Push $2
  Push $3

  StrCpy $LynkExe "$INSTDIR\${MAINBINARYNAME}.exe"
  StrCpy $LynkDesktop "$DESKTOP\${PRODUCTNAME}.lnk"
  StrCpy $LynkIcon "$INSTDIR\${MAINBINARYNAME}-${LYNK_ICON_TAG}.ico"
  File "/oname=$LynkIcon" "${LYNK_ICON_SRC}"

  !if "${STARTMENUFOLDER}" != ""
    !insertmacro LYNK_SET_SHORTCUT_ICON "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !else
    !insertmacro LYNK_SET_SHORTCUT_ICON "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
  !insertmacro LYNK_SET_SHORTCUT_ICON "$LynkDesktop"
  !insertmacro LYNK_SET_SHORTCUT_ICON "$QUICKLAUNCH\User Pinned\TaskBar\${PRODUCTNAME}.lnk"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$LynkIcon$\""

  ; Copies of an earlier icon; the shortcuts above no longer use them.
  FindFirst $0 $1 "$INSTDIR\${MAINBINARYNAME}-*.ico"
  ${DoWhile} $1 != ""
    ${If} "$INSTDIR\$1" != "$LynkIcon"
      Delete "$INSTDIR\$1"
    ${EndIf}
    FindNext $0 $1
  ${Loop}
  FindClose $0

  Pop $3
  Pop $2
  Pop $1
  Pop $0
!macroend

; Tauri deletes lynk's own files and then the folder only if it's empty, so
; take the icon copy along. Updates keep it: the shortcuts point at it.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    Delete "$INSTDIR\${MAINBINARYNAME}-*.ico"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
