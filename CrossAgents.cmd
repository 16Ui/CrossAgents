@echo off
rem CrossAgents launcher: opens the newest build of the hub and the window.
rem
rem Double-click this (or its desktop copy). If the app is already open it does nothing: the
rem dev server owns port 1420 and a second one would only fight over it.
rem The window's own output goes to %LOCALAPPDATA%\CrossAgents\gui.log.

setlocal
set "ROOT=%~dp0"
set "EXE=%ROOT%target\release\crossagents.exe"

if not exist "%EXE%" (
  echo Missing "%EXE%"
  echo Build it first: cargo build --release
  pause
  exit /b 1
)

rem Already running: the window loads the dev server on 1420.
netstat -ano | findstr /C:":1420 " | findstr /I "LISTENING" >nul 2>&1
if not errorlevel 1 exit /b 0

rem The hub is what the window talks to; `hub start` returns once the port answers.
"%EXE%" hub start >nul 2>&1

start "CrossAgents" /min "%ROOT%scripts\dev-window.cmd"
endlocal
