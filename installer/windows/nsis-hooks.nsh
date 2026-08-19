; T067 — registro/desregistro do filtro DirectShow próprio do CamLink
; (camlink_lib.dll, bundle.resources coloca na raiz do $INSTDIR — mesma
; pasta do .exe). Registro é em HKEY_CURRENT_USER (sem elevação — ver
; DllRegisterServer em src-tauri/src/virtualcam/dshow.rs), então o
; /s (silent) do regsvr32 não deveria pedir UAC extra além do que o
; instalador já pede para si mesmo.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registrando filtro DirectShow do CamLink..."
  ExecWait '"$SYSDIR\regsvr32.exe" /s "$INSTDIR\camlink_lib.dll"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removendo registro do filtro DirectShow do CamLink..."
  ExecWait '"$SYSDIR\regsvr32.exe" /u /s "$INSTDIR\camlink_lib.dll"'
!macroend
