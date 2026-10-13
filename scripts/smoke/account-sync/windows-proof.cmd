@echo off
setlocal
if "%~1"=="" (
  echo Usage: windows-proof.cmd path-to-private-proof-config.json
  exit /b 2
)
set "WRITER_PROOF_CONFIG=%~f1"
set "WRITER_PROOF_RECEIPT=%~dp1proof-result.log"
echo Writer Google sign-in proof
echo Complete sign-in in the browser that opens. No Writer content will upload.
"%ProgramFiles%\nodejs\node.exe" "%~dp0desktop.mjs" "%WRITER_PROOF_CONFIG%" > "%WRITER_PROOF_RECEIPT%" 2>&1
set "WRITER_PROOF_EXIT=%ERRORLEVEL%"
type "%WRITER_PROOF_RECEIPT%"
echo.
echo This window can be closed after reading the result.
pause
exit /b %WRITER_PROOF_EXIT%
