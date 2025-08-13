@echo off
chcp 65001 >nul
title Remote Controller Server

echo ================================
echo    远程控制服务端启动器
echo ================================
echo.

:: 检查Python是否安装
python --version >nul 2>&1
if %errorlevel% neq 0 (
    echo [错误] 未找到Python，请先安装Python 3.8或更高版本
    pause
    exit /b 1
)

:: 检查虚拟环境是否存在
if not exist "venv" (
    echo [信息] 创建虚拟环境...
    python -m venv venv
    if %errorlevel% neq 0 (
        echo [错误] 创建虚拟环境失败
        pause
        exit /b 1
    )
)

:: 激活虚拟环境
echo [信息] 激活虚拟环境...
call venv\Scripts\activate.bat

:: 安装依赖
echo [信息] 安装依赖包...
pip install -r requirements.txt
if %errorlevel% neq 0 (
    echo [错误] 安装依赖失败
    pause
    exit /b 1
)

:: 检查.env文件
if not exist ".env" (
    echo [信息] 创建默认配置文件...
    echo HOST=0.0.0.0> .env
    echo PORT=5000>> .env
    echo SECRET_KEY=your-secret-key-change-this>> .env
    echo ADMIN_PASSWORD=admin123>> .env
    echo DEBUG=True>> .env
    echo.
    echo [注意] 请修改.env文件中的SECRET_KEY和ADMIN_PASSWORD
)

echo.
echo [信息] 启动服务器...
echo [提示] 浏览器访问 http://localhost:5000
echo [提示] 默认密码: admin123
echo [提示] 按 Ctrl+C 停止服务器
echo.

python main.py

pause