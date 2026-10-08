@echo off
REM xitter-dl - run a command inside a configured dev shell.
REM
REM   scripts\dev.cmd cargo build
REM   scripts\dev.cmd cargo run -p xitter-dl-cli -- search "rust gui"
REM   scripts\dev.cmd pnpm --dir ui run dev
REM
REM Exists because this host blocks all unsigned local .ps1 files, so the
REM environment script cannot simply be dot-sourced from a default shell.
REM This wrapper sets process-scope Bypass (this process only - it never
REM touches CurrentUser or LocalMachine policy) and then runs your command.
REM
REM Keep this file ASCII-only and CRLF-terminated. cmd.exe reads .cmd files in
REM the OEM codepage, so a UTF-8 em dash becomes mojibake, and an LF-only file
REM misparses badly enough to emit spurious "not recognized" errors.

setlocal
set "REPO=%~dp0.."

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command ^
  ". '%REPO%\scripts\dev-env.ps1'; & %*"
exit /b %ERRORLEVEL%
