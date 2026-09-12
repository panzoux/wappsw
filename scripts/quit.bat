@echo off
rem wappsw has no tray icon and no in-app quit command by design -- this is
rem the supported way to stop it.
taskkill /F /IM wappsw.exe
