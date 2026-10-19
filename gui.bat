@echo off
chcp 65001 >nul
cd /d "%~dp0"

echo.
echo === Aurtrack 窗口 ===
echo.

echo [1/2] 编译（第一次会比较久）...
cargo build -p aurtrack-desktop
if errorlevel 1 (
    echo 编译失败，把上面的错误发给我。
    pause
    exit /b 1
)

echo [2/2] 启动...
start "" "target\debug\aurtrack-desktop.exe"
