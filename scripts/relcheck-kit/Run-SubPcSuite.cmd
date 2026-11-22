@echo off
setlocal
cd /d "%~dp0..\.."
if errorlevel 1 (
    echo EXIT=2
    exit /b 2
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%CD%\scripts\ui-smoke.ps1" -Scenario ReleaseChecks -FromKit -InteractiveApproved
set "relcheck_exit=%ERRORLEVEL%"
echo EXIT=%relcheck_exit%
exit /b %relcheck_exit%
