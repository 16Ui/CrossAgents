@echo off
rem Runs the GUI in dev mode (vite + electron) with its output captured, so a launcher that
rem starts it minimised still leaves something to read when it fails.
rem Called by CrossAgents.cmd; on its own it behaves like `npm run electron:dev`.

setlocal
rem The environment this can be launched from injects its own NODE_OPTIONS: an app-shim that
rem makes "electron ." run as plain Node (its `app` is undefined and the tray crashes on
rem `app.on`) and blocks vite's dep-cache cleanup with a bulk-delete guard. The dev window
rem wants a plain node, so the inherited value is dropped rather than guessed around.
set "NODE_OPTIONS="
set "ELECTRON_RUN_AS_NODE="

set "LOG=%LOCALAPPDATA%\CrossAgents"
if not exist "%LOG%" mkdir "%LOG%"
cd /d "%~dp0..\apps\gui" || exit /b 1

echo ==== %DATE% %TIME% starting ==== >> "%LOG%\gui.log"
call npm run electron:dev >> "%LOG%\gui.log" 2>&1
echo ==== %DATE% %TIME% exited with %ERRORLEVEL% ==== >> "%LOG%\gui.log"
endlocal
