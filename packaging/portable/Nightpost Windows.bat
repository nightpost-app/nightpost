@echo off
rem Starts Nightpost from this key: its data stay in data\, next to this file.
set NIGHTPOST_PORTABLE=1
set "NIGHTPOST_DIR=%~dp0data"
start "" "%~dp0windows\nightpost-gui.exe"
