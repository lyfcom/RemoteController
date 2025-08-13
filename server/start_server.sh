#!/bin/bash

echo "================================"
echo "    远程控制服务端启动器"
echo "================================"
echo

# 检查Python是否安装
if ! command -v python3 &> /dev/null; then
    echo "[错误] 未找到Python3，请先安装Python 3.8或更高版本"
    exit 1
fi

# 检查虚拟环境是否存在
if [ ! -d "venv" ]; then
    echo "[信息] 创建虚拟环境..."
    python3 -m venv venv
    if [ $? -ne 0 ]; then
        echo "[错误] 创建虚拟环境失败"
        exit 1
    fi
fi

# 激活虚拟环境
echo "[信息] 激活虚拟环境..."
source venv/bin/activate

# 安装依赖
echo "[信息] 安装依赖包..."
pip install -r requirements.txt
if [ $? -ne 0 ]; then
    echo "[错误] 安装依赖失败"
    exit 1
fi

# 检查.env文件
if [ ! -f ".env" ]; then
    echo "[信息] 创建默认配置文件..."
    cat > .env << EOF
HOST=0.0.0.0
PORT=5000
SECRET_KEY=your-secret-key-change-this
ADMIN_PASSWORD=admin123
DEBUG=True
EOF
    echo
    echo "[注意] 请修改.env文件中的SECRET_KEY和ADMIN_PASSWORD"
fi

echo
echo "[信息] 启动服务器..."
echo "[提示] 浏览器访问 http://localhost:5000"
echo "[提示] 默认密码: admin123"
echo "[提示] 按 Ctrl+C 停止服务器"
echo

python main.py