@echo off
chcp 65001 >nul
cd /d "%~dp0"

echo.
echo === Aurtrack 无头演示 ===
echo.

where cargo >nul 2>nul
if errorlevel 1 (
    echo 找不到 cargo。请先安装 Rust，或把 cargo 加进 PATH。
    pause
    exit /b 1
)

echo [1/3] 编译...
cargo build --workspace --quiet
if errorlevel 1 (
    echo 编译失败，把上面的错误发给我。
    pause
    exit /b 1
)

echo [2/3] 渲染 demo.wav...
target\debug\aurtrack.exe --stdin < examples\demo.jsonl
if errorlevel 1 (
    echo 渲染失败。
    pause
    exit /b 1
)

echo [3/3] 打开播放器...
start "" "demo.wav"

echo.
echo 完成。demo.wav 在 %CD%
pause
