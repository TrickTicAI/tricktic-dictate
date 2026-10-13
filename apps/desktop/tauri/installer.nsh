; User data (%LOCALAPPDATA%\com.tricktic.dictate) is outside $INSTDIR. NSIS
; never removes it.

; Install identity: the install folder and the uninstall and install-location
; registry keys. These are what Tauri derives for com.tricktic.dictate; keep
; them, since every installed copy finds its next update through them
; (docs/release/identity.md).
!define INSTALLNAME "TrickTic Dictate"
!define INSTALLMANUFACTURER "TrickTic"

; The Metrix Writer identity of 0.39 and earlier, carried over and removed by
; WriterRemoveLegacyInstall. Its data folder is moved by the app on first launch.
!define LEGACYINSTALLNAME "Metrix Writer"
!define LEGACYUNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${LEGACYINSTALLNAME}"
!define LEGACYMANUKEY "Software\metrix"
!define LEGACYMANUPRODUCTKEY "${LEGACYMANUKEY}\${LEGACYINSTALLNAME}"
!define LEGACYBINARYNAME "metrix-writer-desktop.exe"
; Launch-at-login task and Run value names, current and Metrix Writer.
!define STARTUPNAME "TrickTicDictate"
!define LEGACYSTARTUPNAME "MetrixWriter"

!macro WriterRefuseWhileRunning action
  ; The single-instance/data lease also protects portable launches. Refuse while
  ; any generation of Writer is running, rather than force terminating audio.
  !insertmacro WriterRefuseIfRunning "MetrixWriter.exe" "${action}"
  !insertmacro WriterRefuseIfRunning "${LEGACYBINARYNAME}" "${action}"
  !insertmacro WriterRefuseIfRunning "${MAINBINARYNAME}.exe" "${action}"
!macroend
!macro WriterRefuseIfRunning binary action
  nsis_tauri_utils::FindProcessCurrentUser "${binary}"
  Pop $R0
  ${If} $R0 == 0
    Abort "Quit ${PRODUCTNAME} before ${action}. Saved audio will be retained."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro WriterRefuseWhileRunning "updating"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro WriterRefuseWhileRunning "uninstalling"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro WriterRemoveLegacyInstall
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Launch at login is a logon task, not a Run entry, so uninstalling the files
  ; would otherwise leave a task pointing at a program that no longer exists.
  ; Both are removed under both names; a missing one is not an error worth
  ; stopping an uninstall.
  nsExec::Exec 'schtasks.exe /Delete /TN "${STARTUPNAME}" /F'
  Pop $R0
  nsExec::Exec 'schtasks.exe /Delete /TN "${LEGACYSTARTUPNAME}" /F'
  Pop $R0
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${STARTUPNAME}"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${LEGACYSTARTUPNAME}"
!macroend

; A Start Menu, desktop or taskbar shortcut that opens the Metrix Writer install
; ($R1) is replaced by one for this installation. Taskbar pins keep their file
; and only change target, so the pin stays where the user put it.
!macro WriterRepointShortcut lnk new
  !insertmacro IsShortcutTarget "${lnk}" "$R1\${LEGACYBINARYNAME}"
  Pop $0
  ${If} $0 = 1
    Delete "${lnk}"
    ${IfNot} ${FileExists} "${new}"
      CreateShortcut "${new}" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "${new}"
    ${EndIf}
  ${EndIf}
!macroend
!macro WriterRepointPin lnk
  !insertmacro IsShortcutTarget "${lnk}" "$R1\${LEGACYBINARYNAME}"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "${lnk}" "$INSTDIR\${MAINBINARYNAME}.exe"
  ${EndIf}
!macroend

; Runs after this installation is complete, so a failure here never leaves the
; user without a working app. Only Writer's own files are deleted, and only when
; the old folder still holds the old executable; the uninstall entry goes only
; after the executable is gone, so a partial removal can still be uninstalled.
!macro WriterRemoveLegacyInstall
  ReadRegStr $R1 SHCTX "${LEGACYMANUPRODUCTKEY}" ""
  ${If} $R1 == ""
    ReadRegStr $R1 SHCTX "${LEGACYUNINSTKEY}" "InstallLocation"
    ${If} $R1 != ""
      StrCpy $R2 $R1 1
      ${If} $R2 == '"'
        StrCpy $R1 $R1 -1 1
      ${EndIf}
    ${EndIf}
  ${EndIf}
  ${If} $R1 != ""
  ${AndIf} ${FileExists} "$R1\${LEGACYBINARYNAME}"
    !insertmacro WriterRepointShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    !insertmacro WriterRepointShortcut "$SMPROGRAMS\${LEGACYINSTALLNAME}.lnk" "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    !insertmacro WriterRepointShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$DESKTOP\${PRODUCTNAME}.lnk"
    !insertmacro WriterRepointShortcut "$DESKTOP\${LEGACYINSTALLNAME}.lnk" "$DESKTOP\${PRODUCTNAME}.lnk"
    !insertmacro WriterRepointPin "$APPDATA\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\${PRODUCTNAME}.lnk"
    !insertmacro WriterRepointPin "$APPDATA\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\${LEGACYINSTALLNAME}.lnk"
    ClearErrors
    Delete "$R1\${LEGACYBINARYNAME}"
    ${IfNot} ${Errors}
    ${AndIf} $R1 != $INSTDIR
      RMDir /r "$R1\runtime"
      RMDir /r "$R1\notices"
      Delete "$R1\runtime-manifest.json"
      RMDir /r "$R1\${WRITERSTAGED}"
      RMDir /r "$R1\${WRITERRETIRED}"
      Delete "$R1\uninstall.exe"
      ; Removed only when empty: anything else the user put there stays.
      RMDir "$R1"
    ${EndIf}
    ${IfNot} ${FileExists} "$R1\${LEGACYBINARYNAME}"
      DeleteRegKey SHCTX "${LEGACYUNINSTKEY}"
      DeleteRegKey SHCTX "${LEGACYMANUPRODUCTKEY}"
      DeleteRegKey /ifempty SHCTX "${LEGACYMANUKEY}"
    ${EndIf}
  ${EndIf}
!macroend

; MW-SEC-26 W-13/W-14: the new version is extracted completely into
; $INSTDIR\.writer-staged, then Writer's own items are swapped in by renames.
; A killed install before the swap leaves the previous version untouched and
; runnable; the next run discards the partial stage. Swapping whole folders
; also removes files a new version no longer ships (the retired FFmpeg DLLs).
; Only these names are Writer's; anything else in the folder is left alone.
!define WRITERSTAGED ".writer-staged"
!define WRITERRETIRED ".writer-retired"
Var WriterSwapFailed

!macro WriterRestoreItem name
  ${IfNot} ${FileExists} "$INSTDIR\${name}"
  ${AndIf} ${FileExists} "$INSTDIR\${WRITERRETIRED}\${name}"
    Rename "$INSTDIR\${WRITERRETIRED}\${name}" "$INSTDIR\${name}"
  ${EndIf}
!macroend

!macro WriterRestore
  ; The main executable goes back last, so it never runs beside a missing runtime.
  !insertmacro WriterRestoreItem "runtime"
  !insertmacro WriterRestoreItem "notices"
  !insertmacro WriterRestoreItem "runtime-manifest.json"
  !insertmacro WriterRestoreItem "${MAINBINARYNAME}.exe"
!macroend

; Before extracting: undo a swap that was cut short, then clear old leftovers.
!macro WriterPrepareStage
  !insertmacro WriterRestore
  RMDir /r "$INSTDIR\${WRITERRETIRED}"
  RMDir /r "$INSTDIR\${WRITERSTAGED}"
  CreateDirectory "$INSTDIR\${WRITERSTAGED}"
!macroend

!macro WriterRetireItem name
  ${If} $WriterSwapFailed = 0
  ${AndIf} ${FileExists} "$INSTDIR\${name}"
    ClearErrors
    Rename "$INSTDIR\${name}" "$INSTDIR\${WRITERRETIRED}\${name}"
    ${IfThen} ${Errors} ${|} StrCpy $WriterSwapFailed 1 ${|}
  ${EndIf}
!macroend

!macro WriterPlaceItem name
  ${If} $WriterSwapFailed = 0
  ${AndIf} ${FileExists} "$INSTDIR\${WRITERSTAGED}\${name}"
    ClearErrors
    Rename "$INSTDIR\${WRITERSTAGED}\${name}" "$INSTDIR\${name}"
    ${IfThen} ${Errors} ${|} StrCpy $WriterSwapFailed 1 ${|}
  ${EndIf}
!macroend

!macro WriterUnplaceItem name
  ${If} ${FileExists} "$INSTDIR\${name}"
  ${AndIfNot} ${FileExists} "$INSTDIR\${WRITERSTAGED}\${name}"
  ${AndIf} ${FileExists} "$INSTDIR\${WRITERRETIRED}\${name}"
    Rename "$INSTDIR\${name}" "$INSTDIR\${WRITERSTAGED}\${name}"
  ${EndIf}
!macroend

!macro WriterUnplace
  !insertmacro WriterUnplaceItem "${MAINBINARYNAME}.exe"
  !insertmacro WriterUnplaceItem "runtime"
  !insertmacro WriterUnplaceItem "notices"
  !insertmacro WriterUnplaceItem "runtime-manifest.json"
!macroend

!macro WriterSwap
  StrCpy $WriterSwapFailed 0
  CreateDirectory "$INSTDIR\${WRITERRETIRED}"
  ; The old executable leaves first and the new one arrives last.
  !insertmacro WriterRetireItem "${MAINBINARYNAME}.exe"
  !insertmacro WriterRetireItem "runtime"
  !insertmacro WriterRetireItem "notices"
  !insertmacro WriterRetireItem "runtime-manifest.json"
  !insertmacro WriterPlaceItem "runtime"
  !insertmacro WriterPlaceItem "notices"
  !insertmacro WriterPlaceItem "runtime-manifest.json"
  !insertmacro WriterPlaceItem "${MAINBINARYNAME}.exe"
  ${If} $WriterSwapFailed = 1
    ; Put the new items back into the stage, then restore the previous version.
    ${If} ${FileExists} "$INSTDIR\${WRITERSTAGED}\*.*"
      !insertmacro WriterUnplace
    ${EndIf}
    !insertmacro WriterRestore
    Abort "${PRODUCTNAME} could not replace its files; the previous version is unchanged. Close any program using the install folder and try again."
  ${EndIf}
  RMDir /r "$INSTDIR\${WRITERRETIRED}"
  RMDir /r "$INSTDIR\${WRITERSTAGED}"
!macroend
