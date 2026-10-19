@echo off
chcp 65001 >nul
cd /d "%~dp0"

echo.
echo === Aurtrack 窗口 ===
echo.

echo [1/3] 安装前端依赖...
call pnpm install
if errorlevel 1 (
    echo 安装失败，把上面的错误发给我。
    pause
    exit /b 1
)

echo [2/3] 构建前端...
call pnpm --filter @aurtrack/ui build
if errorlevel 1 (
    echo 前端构建失败，把上面的错误发给我。
    pause
    exit /b 1
)

echo [3/3] 编译并启动...
cargo build -p aurtrack-desktop
if errorlevel 1 (
    echo 编译失败，把上面的错误发给我。
    pause
    exit /b 1
)

start "" "target\debug\aurtrack-desktop.exe"
