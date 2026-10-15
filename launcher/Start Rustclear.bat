@echo off
rem Double-click to start Rustclear. Runs start.ps1 next to this file.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1"
if errorlevel 1 pause
