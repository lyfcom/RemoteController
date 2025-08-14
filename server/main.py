from flask import Flask, render_template, request, session, redirect, url_for, send_from_directory
from flask_socketio import SocketIO, emit, disconnect, join_room, leave_room
import os
from dotenv import load_dotenv
import logging
from datetime import datetime
import json

# 加载环境变量
load_dotenv()

# 初始化Flask应用
app = Flask(__name__)
app.config['SECRET_KEY'] = os.getenv('SECRET_KEY', 'your-secret-key-here')

# 初始化SocketIO - 使用最新版本配置
socketio = SocketIO(
    app,
    cors_allowed_origins="*",  # 开发环境允许所有来源
    async_mode='eventlet',     # 指定使用eventlet异步模式
    logger=False,              # 关闭 Socket.IO 层面的详细日志（避免打印大报文）
    engineio_logger=False,     # 关闭 Engine.IO 层面的详细日志（避免打印大报文）
    ping_timeout=120,          # 适当放宽心跳超时，防止长时间操作导致误判
    ping_interval=25,          # 心跳间隔保持默认或适当
    max_http_buffer_size=30*1024*1024  # 增大消息缓冲区到30MB支持截图传输
)

# 配置日志
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)
# 文本清理，去除客户端标记行与提示符行
def _sanitize_output_text(text: str) -> str:
    try:
        if not text:
            return text
        text = text.replace('\r\n', '\n')
        cleaned_lines = []
        for line in text.split('\n'):
            if not line:
                cleaned_lines.append(line)
                continue
            if line.startswith('__RC_END__:'):
                continue
            # 过滤可能的 PowerShell 提示符行，例如: PS C:\path>
            if line.startswith('PS ') and line.endswith('>'):
                continue
            cleaned_lines.append(line)
        return '\n'.join(cleaned_lines).strip()
    except Exception:
        return text

# 全局变量存储连接的客户端信息
connected_clients = {}  # {sid: {uuid: str, connect_time: datetime, ip: str}}
client_uuid_mapping = {}  # {uuid: sid} 用于通过UUID快速查找sid

# 服务器端下载保存目录（使用绝对路径，默认放在应用根目录下的 downloads/）
_default_download = os.path.join(app.root_path, 'downloads')
DOWNLOAD_DIR = os.path.abspath(os.getenv('DOWNLOAD_DIR', _default_download))

# 简单的认证密码
ADMIN_PASSWORD = os.getenv('ADMIN_PASSWORD', 'admin123')

@app.route('/')
def index():
    """主页路由 - 需要认证"""
    if not session.get('authenticated'):
        return redirect(url_for('login'))
    return render_template('index.html', clients=get_client_list())

@app.route('/login', methods=['GET', 'POST'])
def login():
    """登录页面"""
    if request.method == 'POST':
        password = request.form.get('password')
        if password == ADMIN_PASSWORD:
            session['authenticated'] = True
            return redirect(url_for('index'))
        else:
            return render_template('login.html', error='密码错误')
    return render_template('login.html')

@app.route('/logout')
def logout():
    """登出"""
    session.pop('authenticated', None)
    return redirect(url_for('login'))

def get_client_list():
    """获取当前连接的客户端列表（按UUID去重，仅返回最新SID对应的客户端）"""
    clients = []
    for uuid, sid in client_uuid_mapping.items():
        info = connected_clients.get(sid)
        if not info:
            continue
        clients.append({
            'uuid': uuid,
            'connect_time': info['connect_time'].strftime('%Y-%m-%d %H:%M:%S'),
            'ip': info.get('ip', 'Unknown')
        })
    return clients

# ============ WebSocket 事件处理器 ============

@socketio.on('connect')
def handle_connect(auth):
    """客户端连接事件（支持从auth载荷读取UUID）"""
    logger.info(f'客户端连接: {request.sid} auth={auth}')
    # 从请求中获取客户端IP
    client_ip = request.environ.get('HTTP_X_FORWARDED_FOR', request.environ.get('REMOTE_ADDR', 'Unknown'))
    
    # 临时存储连接信息，等待客户端注册UUID
    client_uuid = None
    if isinstance(auth, dict):
        client_uuid = auth.get('uuid')

    connected_clients[request.sid] = {
        'uuid': client_uuid,
        'connect_time': datetime.now(),
        'ip': client_ip,
        'type': 'agent'  # 默认标记为代理客户端，Web端会在join_web_client中覆盖
    }
    if client_uuid:
        # 若已有旧 SID，移除旧记录并断开旧连接，确保不会在前端出现重复客户端
        old_sid = client_uuid_mapping.get(client_uuid)
        if old_sid and old_sid != request.sid:
            try:
                if old_sid in connected_clients:
                    del connected_clients[old_sid]
                # 主动断开旧SID（若仍存活）
                disconnect(old_sid)
            except Exception as e:
                logger.error(f'清理旧连接失败: {e}')
        # 覆盖为最新 SID
        client_uuid_mapping[client_uuid] = request.sid
        logger.info(f'客户端注册(握手auth): {client_uuid} (SID: {request.sid})')
        emit_client_list_update()

@socketio.on('disconnect')
def handle_disconnect():
    """客户端断开连接事件"""
    logger.info(f'客户端断开连接: {request.sid}')
    
    # 从连接列表中移除
    if request.sid in connected_clients:
        client_info = connected_clients[request.sid]
        if client_info['uuid']:
            # 仅当映射仍指向本次断开的 SID 时才移除，避免覆盖新连接
            current_sid = client_uuid_mapping.get(client_info['uuid'])
            if current_sid == request.sid:
                client_uuid_mapping.pop(client_info['uuid'], None)
        del connected_clients[request.sid]
    
    # 通知所有Web客户端更新客户端列表
    emit_client_list_update()

@socketio.on('register_client')
def handle_client_register(data):
    """兼容保留：处理客户端UUID注册（如果客户端未通过auth提供）"""
    try:
        client_uuid = data.get('uuid')
        if not client_uuid:
            emit('error', {'message': '缺少UUID'})
            return

        logger.info(f'客户端注册(事件): {client_uuid} (SID: {request.sid})')
        if request.sid in connected_clients:
            connected_clients[request.sid]['uuid'] = client_uuid
            connected_clients[request.sid]['type'] = 'agent'
            client_uuid_mapping[client_uuid] = request.sid
            emit('register_success', {'message': '注册成功'})
            emit_client_list_update()
        else:
            emit('error', {'message': '连接信息不存在'})
    except Exception as e:
        logger.error(f'客户端注册失败: {e}')
        emit('error', {'message': f'注册失败: {str(e)}'})

@socketio.on('command_output')
def handle_command_output(data):
    """处理来自客户端的命令执行结果"""
    try:
        client_uuid = data.get('uuid')
        output_raw = data.get('output', '')
        error_raw = data.get('error', '')
        # 保持控制台体验：不强制 strip，保留 \n；仅去掉我们自己的结束标记与 PS 提示符
        output = _sanitize_output_text(output_raw)
        error = _sanitize_output_text(error_raw)
        command = data.get('command', '')
        
        logger.info(f'收到客户端 {client_uuid} 的命令输出')
        
        # 转发给所有Web客户端
        socketio.emit('command_response', {
            'uuid': client_uuid,
            'command': command,
            'output': output,
            'error': error,
            'timestamp': datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        }, room='web_clients')
        
    except Exception as e:
        logger.error(f'处理命令输出失败: {e}')

@socketio.on('file_operation_result')
def handle_file_operation_result(data):
    """处理来自客户端的文件操作结果"""
    try:
        client_uuid = data.get('uuid')
        operation = data.get('operation')
        success = data.get('success', False)
        result_data = data.get('data', {})
        error = data.get('error', '')
        
        logger.info(f'收到客户端 {client_uuid} 的文件操作结果: {operation}')
        
        # 转发给所有Web客户端
        socketio.emit('file_operation_response', {
            'uuid': client_uuid,
            'operation': operation,
            'success': success,
            'data': result_data,
            'error': error,
            'timestamp': datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        }, room='web_clients')
        
    except Exception as e:
        logger.error(f'处理文件操作结果失败: {e}')

@socketio.on('upload_file_result')
def handle_upload_file_result(data):
    """处理来自客户端的上传结果并转发给Web端"""
    try:
        client_uuid = data.get('uuid')
        success = data.get('success', False)
        path = data.get('path', '')
        error = data.get('error', '')
        socketio.emit('upload_file_response', {
            'uuid': client_uuid,
            'success': success,
            'path': path,
            'error': error,
            'timestamp': datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        }, room='web_clients')
    except Exception as e:
        logger.error(f'处理上传结果失败: {e}')

@socketio.on('download_file_result')
def handle_download_file_result(data):
    """处理来自客户端的下载结果并转发给Web端"""
    try:
        client_uuid = data.get('uuid')
        success = data.get('success', False)
        path = data.get('path', '')
        file_base64 = data.get('file_base64', '')
        error = data.get('error', '')
        download_url = None
        if success and file_base64:
            try:
                # 保存到服务器下载目录
                os.makedirs(DOWNLOAD_DIR, exist_ok=True)
                base_name = os.path.basename(path) or 'downloaded.bin'
                ts = datetime.now().strftime('%Y%m%d_%H%M%S')
                safe_name = f"{ts}_{base_name}"
                import base64
                data_bytes = base64.b64decode(file_base64)
                server_path = os.path.join(DOWNLOAD_DIR, safe_name)
                with open(server_path, 'wb') as f:
                    f.write(data_bytes)
                download_url = url_for('download_saved_file', filename=safe_name)
                logger.info(f"客户端下载保存: {server_path} -> {download_url}")
            except Exception as save_e:
                logger.error(f'保存下载文件失败: {save_e}')
                error = f'保存下载文件失败: {save_e}'
                success = False
        socketio.emit('file_download_response', {
            'uuid': client_uuid,
            'success': success,
            'path': path,
            'file_base64': file_base64,
            'download_url': download_url,
            'error': error,
            'timestamp': datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        }, room='web_clients')
    except Exception as e:
        logger.error(f'处理下载结果失败: {e}')
        
@app.route('/download/<path:filename>')
def download_saved_file(filename):
    """提供服务器已保存的下载文件"""
    return send_from_directory(DOWNLOAD_DIR, filename, as_attachment=True)

@socketio.on('screenshot_result')
def handle_screenshot_result(data):
    """处理客户端截图结果，转发给Web端"""
    try:
        client_uuid = data.get('uuid')
        success = data.get('success', False)
        image_base64 = data.get('image_base64', '')
        error = data.get('error', '')
        socketio.emit('screenshot_response', {
            'uuid': client_uuid,
            'success': success,
            'image_base64': image_base64,
            'error': error,
            'timestamp': datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        }, room='web_clients')
    except Exception as e:
        logger.error(f'处理截图结果失败: {e}')

# ============ Web客户端事件处理器 ============

@socketio.on('join_web_client')
def handle_join_web_client():
    """Web客户端加入房间"""
    join_room('web_clients')
    # 标记此连接为web控制台端，以防被误认为agent
    if request.sid in connected_clients:
        connected_clients[request.sid]['type'] = 'web'
    emit('client_list', {'clients': get_client_list()})
    logger.info(f'Web客户端加入: {request.sid}')

@socketio.on('leave_web_client')
def handle_leave_web_client():
    """Web客户端离开房间"""
    leave_room('web_clients')
    logger.info(f'Web客户端离开: {request.sid}')

@socketio.on('execute_command')
def handle_execute_command(data):
    """处理来自Web客户端的命令执行请求"""
    try:
        target_uuid = data.get('target_uuid')
        command = data.get('command')
        use_shared_context = data.get('use_shared_context', True)
        
        if not target_uuid or not command:
            emit('error', {'message': '缺少目标UUID或命令'})
            return
        
        # 查找目标客户端
        target_sid = client_uuid_mapping.get(target_uuid)
        if not target_sid:
            emit('error', {'message': f'客户端 {target_uuid} 未连接'})
            return
        
        logger.info(f'发送命令到客户端 {target_uuid}: {command}')
        
        # 发送命令到目标客户端
        socketio.emit('run_command', {
            'command': command,
            'use_shared_context': use_shared_context
        }, room=target_sid)
        
        # 通知Web客户端命令已发送
        emit('command_sent', {
            'target_uuid': target_uuid,
            'command': command,
            'timestamp': datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        })
        
    except Exception as e:
        logger.error(f'执行命令失败: {e}')
        emit('error', {'message': f'执行命令失败: {str(e)}'})

@socketio.on('file_operation')
def handle_file_operation(data):
    """处理来自Web客户端的文件操作请求"""
    try:
        target_uuid = data.get('target_uuid')
        operation = data.get('operation')
        path = data.get('path', '')
        file_data = data.get('file_data', '')
        
        if not target_uuid or not operation:
            emit('error', {'message': '缺少目标UUID或操作类型'})
            return
        
        # 查找目标客户端
        target_sid = client_uuid_mapping.get(target_uuid)
        if not target_sid:
            emit('error', {'message': f'客户端 {target_uuid} 未连接'})
            return
        
        logger.info(f'发送文件操作到客户端 {target_uuid}: {operation} - {path}')
        
        # 发送文件操作请求到目标客户端
        socketio.emit('do_file_operation', {
            'operation': operation,
            'path': path,
            'file_data': file_data
        }, room=target_sid)
        
        # 通知Web客户端请求已发送
        emit('file_operation_sent', {
            'target_uuid': target_uuid,
            'operation': operation,
            'path': path,
            'timestamp': datetime.now().strftime('%Y-%m-%d %H:%M:%S')
        })
        
    except Exception as e:
        logger.error(f'文件操作失败: {e}')
        emit('error', {'message': f'文件操作失败: {str(e)}'})

@socketio.on('restart_client')
def handle_restart_client(data):
    """处理来自Web客户端的重启目标客户端请求"""
    try:
        target_uuid = data.get('target_uuid')
        if not target_uuid:
            emit('error', {'message': '缺少目标UUID'})
            return
        target_sid = client_uuid_mapping.get(target_uuid)
        if not target_sid:
            emit('error', {'message': f'客户端 {target_uuid} 未连接'})
            return
        logger.info(f'发送重启到客户端 {target_uuid}')
        socketio.emit('restart', {}, room=target_sid)
        emit('info', {'message': f'已通知客户端 {target_uuid} 重启'})
    except Exception as e:
        logger.error(f'重启请求失败: {e}')
        emit('error', {'message': f'重启请求失败: {str(e)}'})
@socketio.on('reset_context')
def handle_reset_context(data):
    """处理来自Web客户端的重置共享上下文请求"""
    try:
        target_uuid = data.get('target_uuid')
        if not target_uuid:
            emit('error', {'message': '缺少目标UUID'})
            return
        target_sid = client_uuid_mapping.get(target_uuid)
        if not target_sid:
            emit('error', {'message': f'客户端 {target_uuid} 未连接'})
            return
        logger.info(f'发送上下文重置到客户端 {target_uuid}')
        socketio.emit('reset_context', {}, room=target_sid)
        emit('info', {'message': f'已通知客户端 {target_uuid} 重置共享上下文'})
    except Exception as e:
        logger.error(f'上下文重置请求失败: {e}')
        emit('error', {'message': f'上下文重置请求失败: {str(e)}'})
@socketio.on('screenshot')
def handle_screenshot(data):
    """处理来自Web客户端的截图请求"""
    try:
        target_uuid = data.get('target_uuid')
        display_index = data.get('display_index')
        if not target_uuid:
            emit('error', {'message': '缺少目标UUID'})
            return
        target_sid = client_uuid_mapping.get(target_uuid)
        if not target_sid:
            emit('error', {'message': f'客户端 {target_uuid} 未连接'})
            return
        socketio.emit('screenshot', {
            'display_index': display_index
        }, room=target_sid)
    except Exception as e:
        logger.error(f'截图请求失败: {e}')
        emit('error', {'message': f'截图请求失败: {str(e)}'})

@socketio.on('upload_file_to_client')
def handle_upload_file_to_client(data):
    """处理来自Web客户端的上传请求并转发到目标客户端"""
    try:
        target_uuid = data.get('target_uuid')
        path = data.get('path')  # 目标文件完整路径（含文件名）
        file_base64 = data.get('file_base64')
        if not target_uuid or not path or not file_base64:
            emit('error', {'message': '缺少目标UUID、路径或文件数据'})
            return
        target_sid = client_uuid_mapping.get(target_uuid)
        if not target_sid:
            emit('error', {'message': f'客户端 {target_uuid} 未连接'})
            return
        logger.info(f'转发上传到客户端 {target_uuid}: {path}')
        socketio.emit('upload_file', {
            'path': path,
            'file_base64': file_base64,
        }, room=target_sid)
    except Exception as e:
        logger.error(f'上传转发失败: {e}')
        emit('error', {'message': f'上传转发失败: {str(e)}'})

@socketio.on('download_file_from_client')
def handle_download_file_from_client(data):
    """处理来自Web客户端的下载请求并转发到目标客户端"""
    try:
        target_uuid = data.get('target_uuid')
        path = data.get('path')
        if not target_uuid or not path:
            emit('error', {'message': '缺少目标UUID或路径'})
            return
        target_sid = client_uuid_mapping.get(target_uuid)
        if not target_sid:
            emit('error', {'message': f'客户端 {target_uuid} 未连接'})
            return
        logger.info(f'转发下载到客户端 {target_uuid}: {path}')
        socketio.emit('download_file', {
            'path': path,
        }, room=target_sid)
    except Exception as e:
        logger.error(f'下载转发失败: {e}')
        emit('error', {'message': f'下载转发失败: {str(e)}'})

def emit_client_list_update():
    """向所有Web客户端发送客户端列表更新"""
    try:
        socketio.emit('client_list_update', {
            'clients': get_client_list()
        }, room='web_clients')
        logger.info('已推送客户端列表更新')
    except Exception as e:
        logger.error(f'推送客户端列表更新失败: {e}')

if __name__ == '__main__':
    # 创建模板和静态文件目录
    os.makedirs('templates', exist_ok=True)
    os.makedirs('static', exist_ok=True)
    
    # 启动服务器
    host = os.getenv('HOST', '0.0.0.0')
    port = int(os.getenv('PORT', 5000))
    debug = os.getenv('DEBUG', 'True').lower() == 'true'
    
    # 确保下载目录存在
    try:
        os.makedirs(DOWNLOAD_DIR, exist_ok=True)
    except Exception as e:
        logger.error(f'创建下载目录失败: {e}')
    
    logger.info(f'启动服务器: {host}:{port} (Debug: {debug})')
    socketio.run(app, host=host, port=port, debug=debug)
