#!/bin/sh
# ============================================================================
# macOS 构建产物 ad-hoc 签名修复：清 quarantine → codesign 重签 → 验证
#
# 背景：本机 `tauri build` 产出的 .app 在 bundle 层签名损坏（codesign 显示
# Signature=adhoc, linker-signed 且 Sealed Resources=none），macOS 15
# 双击提示"已损坏"。正常路径由构建命令的 APPLE_SIGNING_IDENTITY=- 让
# bundler 做 ad-hoc 签名（见 package.json 的 build:mac）；本脚本作为兜底，
# 对已有产物强制重签，修复后本机可正常双击启动。
#
# 用法：scripts/fix-macos-sign.sh [.app 路径]
#   例：scripts/fix-macos-sign.sh src-tauri/target/release/bundle/macos/ZBar.app
#   省略参数时自动搜索 src-tauri/target 下全部 bundle/macos 的 .app 逐个修复，
#   兼容 target/release 与 target/<triple>/release（--target 交叉编译）两种布局
#
# 注意：仅适用 ad-hoc 签名（本机自用场景）；对外分发需正式证书 + 公证，
# 不在本脚本范围
# ============================================================================
set -e

# 记录调用方目录，保证显式传入的相对路径在 cd 后仍按原位置解析
ORIG_DIR=$(pwd)
cd "$(dirname "$0")/.."

# 非 macOS（或未装命令行工具）环境没有 codesign，无事可做：提示后按成功
# 退出，不阻塞 npm 串联的构建链
if ! command -v codesign >/dev/null 2>&1; then
  echo "提示：未找到 codesign 命令，跳过 macOS 签名修复（非 macOS 环境属正常现象）"
  exit 0
fi

if [ -n "$1" ]; then
  case "$1" in
    /*) APP_PATH="$1" ;;
    *)  APP_PATH="$ORIG_DIR/$1" ;;
  esac
  if [ ! -d "$APP_PATH" ]; then
    echo "错误：指定的路径不存在或不是目录：$APP_PATH"
    echo "用法：sh scripts/fix-macos-sign.sh [.app 路径]"
    echo "  例：sh scripts/fix-macos-sign.sh src-tauri/target/release/bundle/macos/ZBar.app"
    exit 1
  fi
  APPS="$APP_PATH"
else
  # find 结果按换行切分后整行就是一个路径，含空格也不会被拆散；
  # 通配的 bundle/macos 前缀同时命中 release 与 <triple>/release 布局
  APPS=$(find src-tauri/target -type d -name '*.app' -path '*/bundle/macos/*' 2>/dev/null)
fi

if [ -z "$APPS" ]; then
  echo "错误：未在 src-tauri/target 下找到任何 .app 构建产物。"
  echo "请先构建（npm run build:mac 或 npx tauri build），或显式传入 .app 路径："
  echo "  例：sh scripts/fix-macos-sign.sh src-tauri/target/release/bundle/macos/ZBar.app"
  exit 1
fi

# IFS 置为单个换行：for 按行遍历 find 结果，避免路径含空格被按空格拆分
IFS='
'
for app in $APPS; do
  echo "==> 修复签名：$app"
  # 1. 清除 quarantine 等扩展属性（隔离标记会触发 Gatekeeper 拦截）
  xattr -cr "$app"
  # 2. ad-hoc 强制重签（-s - 表示 ad-hoc）：--deep 连同内嵌二进制一并覆盖
  codesign --force --deep -s - "$app"
  # 3. 校验签名与 Sealed Resources 完整性，失败则 set -e 直接以非零退出
  codesign --verify --deep -v "$app"
  echo "==> 完成：$app 签名校验通过"
done

echo "全部 .app 签名修复完成。"
