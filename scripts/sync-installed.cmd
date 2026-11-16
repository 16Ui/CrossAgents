@echo off
rem Syncs the freshly built app (release\win-unpacked) over the installed copy, so the
rem desktop shortcut -- which points at the install directory and never moves -- keeps
rem opening the newest build without being recreated.
rem
rem Called by `npm run sync-install`, which electron:build runs after packaging.
rem The running app is closed first: Windows locks a running exe's file.
rem The NSIS uninstaller in the install directory is left alone on purpose.

setlocal
set "SRC=%~dp0..\apps\gui\release\win-unpacked"
set "DST=%LOCALAPPDATA%\Programs\crossagents-gui"

if not exist "%SRC%\CrossAgents.exe" (
  echo Nothing to sync: "%SRC%\CrossAgents.exe" is missing. Run npm run electron:build first.
  exit /b 1
)
if not exist "%DST%\CrossAgents.exe" (
  echo Not installed yet: "%DST%" is missing. Run the Setup exe once; after that this
  echo script keeps that copy current on every build.
  exit /b 1
)

rem Force-kill releases the file handle immediately; no wait is needed after it.
taskkill /F /IM CrossAgents.exe >nul 2>&1

robocopy "%SRC%" "%DST%" /E /NFL /NDL /NJH /NJS /NP >nul
if errorlevel 8 (
  echo robocopy failed with code %ERRORLEVEL%
  exit /b 1
)

for %%F in ("%DST%\CrossAgents.exe") do echo Installed copy updated: %%~tF
endlocal
