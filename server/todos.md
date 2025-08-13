# 服务端 (Python) 开发计划

## 1. 项目设置

- [ ] 初始化Python项目环境 (例如，使用 `venv`)
- [ ] 安装依赖: `Flask`, `Flask-SocketIO`, `python-dotenv`
- [ ] 创建基础项目结构: `main.py`, `templates/`, `static/`
- [ ] 设置 `.env` 文件来存储配置 (例如, `HOST`, `PORT`)

## 2. 核心功能

- [ ] **Flask Web服务器**:
    - [ ] 创建一个基本的Flask应用实例。
    - [ ] 设置一个主页路由 (`/`) 用于展示和驱动前端UI。
- [ ] **WebSocket (SocketIO) 集成**:
    - [ ] 初始化SocketIO并将其绑定到Flask应用。
    - [ ] 实现客户端连接池，用于管理和追踪所有连接的客户端 (以UUID为标识)。
- [ ] **客户端管理**:
    - [ ] 实现 `connect` 和 `disconnect` 事件处理器。
    - [ ] 当客户端连接时，要求其注册UUID，并记录其会话ID (`sid`)。
    - [ ] 当客户端断开连接时，从活动列表中移除。
    - [ ] 能通过WebSocket向Web UI实时推送在线客户端列表。

## 3. Web UI (前端)

- [ ] 创建一个简单的 `index.html` 页面作为主控制台。
- [ ] 使用JavaScript连接到后端的SocketIO服务。
- [ ] **UI组件设计**:
    - [ ] **客户端列表**: 动态显示所有在线客户端的UUID，并允许用户选择其中一个进行操作。
    - [ ] **伪终端界面**: 一个只读的文本区域，用于实时显示来自客户端的命令执行结果。
    - [ ] **命令输入框**: 一个输入字段，用于向选定的客户端发送命令。
    - [ ] **上下文切换**: 提供一个复选框或开关，用于决定命令是在“共享上下文”还是“新上下文”中执行。
    - [ ] **文件操作按钮**: 用于触发如“列出目录”、“上传/下载文件”等操作的按钮。

## 4. WebSocket API 设计 (事件驱动)

- **从 Web UI -> 服务端**:
    - `list_clients`: 请求当前所有连接的客户端。
    - `execute_command`: `{ "target_uuid": "...", "command": "...", "use_shared_context": true }`
    - `file_operation`: `{ "target_uuid": "...", "operation": "list_dir", "path": "..." }`
- **从 服务端 -> Web UI**:
    - `update_client_list`: `{ "clients": ["uuid1", "uuid2", ...] }`
    - `command_response`: `{ "uuid": "...", "output": "..." }`
    - `file_operation_response`: `{ "uuid": "...", "data": [...] }`
- **从 客户端 -> 服务端**:
    - `register`: (在 `connect` 事件中处理) 客户端上报 `{ "uuid": "..." }`。
    - `command_output`: 从客户端接收的命令执行结果。
    - `file_operation_result`: 从客户端接收的文件操作结果。
- **从 服务端 -> 客户端**:
    - `run_command`: `{ "command": "...", "use_shared_context": true/false }`
    - `do_file_operation`: `{ "operation": "...", "path": "..." }`

## 5. 增强功能与未来规划

- [ ] **安全性**:
    - [ ] 为Web UI添加一个简单的基于会话的密码认证。
    - [ ] 探索使用 WSS/HTTPS 来加密所有通信。
- [ ] **文件传输**:
    - [ ] 实现文件上传到客户端的功能。
    - [ ] 实现从客户端下载文件的功能。
- [ ] **UI/UX 改进**:
    - [ ] 美化伪终端界面，可能使用 `xterm.js` 库。
    - [ ] 增加客户端状态指示灯 (例如，在线/离线/忙碌)。