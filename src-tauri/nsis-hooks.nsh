; Tauri NSIS 安装器钩子：卸载时清理用户目录下的 MCP 脚本
; （baidupan-mcp.js 由应用写入 %USERPROFILE%，不在安装目录，默认卸载不会删除）
!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$PROFILE\baidupan-mcp.js"
!macroend
