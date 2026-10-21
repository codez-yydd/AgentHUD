#!/usr/bin/env bash
# zbar-sync 启动脚本
# 用法：./start.sh  或  PORT=8080 ./start.sh
#
# 首次运行会自动创建 zbar-data/ 目录、建库、生成 master token。
# 日志会打印 master token，复制到客户端注册设备用。

set -e
cd "$(dirname "$0")"

# 默认端口 3838，可通过环境变量 PORT 覆盖
export PORT="${PORT:-3838}"

echo "启动 zbar-sync（端口 $PORT）..."

# 入口走 python3 app.py 的 app.main()：先完成建库、master token 读取/生成、
# token 打印、自动清理线程启动，再启动 HTTP 服务。（此前用
# python3 -m waitress 只 import 模块拿 Flask 实例，main() 里的初始化全部
# 不执行，触库端点会 500，故弃用。）
# main() 优先用 waitress（生产级 WSGI 服务器）启动：固定 16 线程池 + 60s
# channel_timeout 掐慢连接。Werkzeug dev server（未装 waitress 时的回退，
# 仅限调试）无超时、无并发上限，慢请求线程无限堆积，大窗口查询并发时会
# 把内存打爆；waitress 固定线程池把并发钉在上限。main() 的 waitress 监听
# 地址/端口取 config.HOST/PORT（HOST 环境变量同样生效，不再被
# --listen=0.0.0.0 绕过；本脚本上方已导出 PORT）。
exec python3 app.py
