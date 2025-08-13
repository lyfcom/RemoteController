## 远程控制服务端（Server）技术文档

本目录为基于 Flask + Flask-SocketIO 的 Web 管理端。用于连接/管理 Windows 客户端（Rust 实现），提供命令执行、文件操作、截图与远程重启等能力。

### 1. 功能概览
- 登录鉴权（简单密码，.env 可配）
- 实时客户端列表（按 UUID）
- 命令执行（共享上下文/新上下文）
- 文件操作（列目录、读/写文件、删除文件/目录）
- 屏幕截图（可选显示器索引，PNG Base64）
- 远程重启客户端进程（用于资源重载 config.json / client_id.txt 等）
- 控制台体验优化（命令与输出分离渲染，保留换行）
- 文件上传/下载（浏览器→客户端保存；客户端→服务器保存并生成可下载链接）
- 重置共享上下文（让目标客户端的持久 PowerShell/CMD 会话重新初始化）

### 2. 技术栈
- Python 3.8+
- Flask 3.x
- Flask-SocketIO 5.x（eventlet 驱动）
- 前端：原生 HTML/CSS/JS + Socket.IO 4.7.x

### 3. 目录结构
```
server/
├─ main.py                 # 服务端入口（WS 事件与路由）
├─ requirements.txt        # 依赖
├─ start_server.bat/.sh    # 一键启动脚本
├─ templates/
│  ├─ login.html           # 登录页
│  └─ index.html           # 控制台 UI
└─ .env                    # 运行配置（HOST/PORT/SECRET_KEY/ADMIN_PASSWORD/DEBUG）
```

### 4. 启动与配置
1) Windows：双击 `start_server.bat`
2) Linux/macOS：`chmod +x start_server.sh && ./start_server.sh`
3) 手动：
```
python -m venv venv
venv\Scripts\activate  # 或 source venv/bin/activate
pip install -r requirements.txt
python main.py
```
.env 示例：
```
HOST=0.0.0.0
PORT=5000
SECRET_KEY=replace-me
ADMIN_PASSWORD=replace-me
DEBUG=True
DOWNLOAD_DIR=downloads  # 可选：服务端保存“从客户端下载的文件”的目录（支持绝对路径）
```

### 5. 前端 UI（templates/index.html）
- 客户端列表：选择目标后可执行命令、文件操作、截图、重启
- 控制台渲染：
  - 发送命令仅打印 `$ <cmd>` 行
  - 收到结果只追加输出/错误（保留换行），去除内部结束标记与 PS 提示符
  - 错误以 ERROR: 行分块显示
- 文件上传到客户端：选择本地文件，填写客户端目标保存路径并上传
- 从客户端下载：填写客户端源路径，请求后在页面得到一个下载链接
- 重置共享上下文：一键让客户端的共享 PowerShell/CMD 进程重建

### 6. Socket.IO 事件协议（Server 侧）

服务端接收（来自 Web 控制台）：
- `join_web_client`：加入控制台房间 `web_clients`
- `execute_command`：`{ target_uuid, command, use_shared_context }`
- `file_operation`：`{ target_uuid, operation, path, file_data }`
- `screenshot`：`{ target_uuid, display_index }`
- `restart_client`：`{ target_uuid }` 远程重启客户端
- `reset_context`：`{ target_uuid }` 重置目标客户端的共享上下文
- `upload_file_to_client`：`{ target_uuid, path, file_base64 }` 上传文件到目标客户端指定路径
- `download_file_from_client`：`{ target_uuid, path }` 从目标客户端读取文件

服务端发送（至 Web 控制台）：
- `client_list` / `client_list_update`：`{ clients: [{ uuid, connect_time, ip }] }`
- `command_sent`：`{ target_uuid, command, timestamp }`（仅打印命令）
- `command_response`：`{ uuid, command, output, error, timestamp }`
- `file_operation_response`：`{ uuid, operation, success, data, error, timestamp }`
- `screenshot_response`：`{ uuid, success, image_base64, error, timestamp }`
- `info` / `error`：统一提示
- `upload_file_response`：`{ uuid, success, path, error, timestamp }`
- `file_download_response`：`{ uuid, success, path, file_base64?, download_url?, error, timestamp }`

服务端接收（来自客户端 Agent）：
- `register_client`：`{ uuid }`（兼容事件注册）
- `command_output`：命令执行结果
- `file_operation_result`：文件操作结果
- `screenshot_result`：截图结果
- `upload_file_result`：客户端处理上传的结果
- `download_file_result`：客户端回传下载文件的结果（可能附带 `file_base64`）

服务端发送（至客户端 Agent）：
- `run_command`：`{ command, use_shared_context }`
- `do_file_operation`：`{ operation, path, file_data }`
- `screenshot`：`{ display_index }`
- `restart`：无载荷（触发远程自重启）
- `reset_context`：无载荷（重置共享 PowerShell/CMD 会话）
- `upload_file`：`{ path, file_base64 }`
- `download_file`：`{ path }`

### 7. 下载保存目录与路由
- 目录：`DOWNLOAD_DIR` 环境变量指定（默认 `server/downloads/`，绝对路径优先）；服务端会将“从客户端下载”的文件保存于此目录。
- 路由：`GET /download/<filename>` 从上述目录提供文件下载，用于网页端生成可点击链接。
- 日志：保存成功会打印“客户端下载保存: <绝对路径> -> <URL>”。

注意：若前置了反向代理，请确保放行并转发 `/download/` 前缀到 Flask 应用，否则会出现 404。

### 8. 关键实现说明
- 握手注册：Server `connect(auth)` 支持从握手 `auth.uuid` 接收 UUID 并立即入库；也兼容后续 `register_client` 事件。
- 控制台清洗规则：
  - 去除内部结束标记 `__RC_END__:*`
  - 去除 `PS ...>` 提示符
  - 保留换行与内容，避免过度裁剪
- 错误与信息：统一通过 `error` / `info` 事件推送前端提示。

### 9. 安全与部署建议
- 请修改 `.env` 中 `SECRET_KEY` 与 `ADMIN_PASSWORD`
- 生产建议启用 HTTPS/WSS（通过反向代理）
- 内网可控环境使用，谨慎暴露公网

### 10. 常见问题（FAQ）
- 无客户端显示：确认客户端已上报 UUID（Agent 侧日志）
- 输出格式异常：前端已优化，尝试刷新；或检查客户端是否回传了正确的换行
- 截图失败：Windows 会话/权限问题，请在交互用户会话中运行
 - 下载 404：确认 `DOWNLOAD_DIR` 目录存在且 Flask 进程有写/读权限；同时检查反向代理是否正确转发了 `/download/` 前缀。

### 11. 贡献
- 新增事件：在 `main.py` 中添加服务端事件；在 `index.html` 中绑定 UI 与前端事件；在 Agent 中对齐实现
- 代码风格：保持事件与载荷命名一致，优先复用现有模式
