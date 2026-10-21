#!/usr/bin/env bash
# ============================================================================
# ZBar 发布脚本：版本号同步 → 签名构建 → 生成双源 latest.json → 上传双仓库
#
# 用法:
#   scripts/release.sh 0.2.0                 # 构建并上传 GitHub + Gitee
#   scripts/release.sh 0.2.0 --no-upload     # 只构建不上传（本地验证产物）
#   scripts/release.sh 0.2.0 --no-gitee      # 跳过 Gitee 上传（本机直连 Gitee
#                                            #   大附件易超时，Gitee 侧由
#                                            #   scripts/publish_gitee.sh 直发）
#   scripts/release.sh 0.2.0 --notes "修复XX" # 指定更新说明（默认取参数后全部）
#
#   上传前会校验远程 main 的版本号已同步（创建 release 会在远程 main HEAD
#   自动打 tag，版本号未提交推送会被拦下）：先提交并推送版本号变更再运行
#
# 必需环境:
#   ~/.tauri/zbar-updater.key   更新签名私钥（首次用
#                               `npx tauri signer generate -w ~/.tauri/zbar-updater.key`
#                               生成，公钥已写入 tauri.conf.json。私钥丢失将无法
#                               再推送更新，务必备份！）
#   gh                          GitHub CLI（已登录）；未安装时自动改用 git 凭据
#                               里的 PAT 走 REST API 兜底（需 jq；PAT 也可用
#                               GITHUB_TOKEN 环境变量提供）
#   GITEE_TOKEN                 Gitee 私人令牌（设置→私人令牌，勾选 projects）
#
# 双仓库更新机制:
#   应用内 updater 的 endpoints 依次尝试 GitHub latest.json 与 Gitee 固定 tag
#   的 latest.json；两份 latest.json 内容一致，仅安装包下载 url 指向各自仓库，
#   任一仓库可达即可完成更新。
# ============================================================================
set -euo pipefail

# ---------- 参数 ----------
VERSION="${1:-}"
NOTES=""
UPLOAD=1
SKIP_GITEE=0
shift_done=0
if [[ -z "$VERSION" ]]; then
  echo "用法: scripts/release.sh <版本号> [--no-upload] [--no-gitee] [--notes \"更新说明\"]" >&2
  exit 1
fi
shift || true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-upload) UPLOAD=0 ;;
    --no-gitee) SKIP_GITEE=1 ;;
    --notes) shift; NOTES="${1:-}" ;;
    *) NOTES="$NOTES$1 " ;;
  esac
  shift || true
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "版本号格式应为 x.y.z: $VERSION" >&2; exit 1; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
GITHUB_REPO="codez-yydd/AgentHUD"
GITEE_REPO="codezwx/AgentHUD"
KEY_PATH="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/zbar-updater.key}"
BUNDLE_DIR="$ROOT/src-tauri/target/release/bundle"

[[ -f "$KEY_PATH" ]] || { echo "未找到更新签名私钥: $KEY_PATH（见脚本头部说明）" >&2; exit 1; }

echo "==> 1/6 同步版本号到 $VERSION"
# 三处版本号：package.json / tauri.conf.json / Cargo.toml（tauri 打包以 tauri.conf.json 为准）
# 用 perl 替换而非 sed：BSD/GNU sed 的 -i 与 0,/addr/ 语法在 macOS 上不兼容
perl -pi -e 's/"version": "[^"]*"/"version": "'"$VERSION"'"/' package.json src-tauri/tauri.conf.json
perl -pi -e 'if (!$done && /^version = /) { s/version = "[^"]*"/version = "'"$VERSION"'"/; $done = 1 }' src-tauri/Cargo.toml

echo "==> 2/6 安装依赖"
npm install --silent

echo "==> 3/6 签名构建（产物含 .sig 签名文件）"
# TAURI_SIGNING_PRIVATE_KEY 可直接传私钥文件路径（tauri CLI 自动识别）
export TAURI_SIGNING_PRIVATE_KEY="$KEY_PATH"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
npm run tauri build

# ---------- 收集产物 ----------
NSIS_EXE=$(ls "$BUNDLE_DIR/nsis/"*x64-setup.exe 2>/dev/null | head -1 || true)
MSI=$(ls "$BUNDLE_DIR/msi/"*.msi 2>/dev/null | head -1 || true)
MAC_TAR=$(ls "$BUNDLE_DIR/macos/"*.app.tar.gz 2>/dev/null | head -1 || true)
# Windows 构建必有 NSIS；在 mac 上构建时只有 mac 产物（用于补传 macOS 更新包）
if [[ -z "$NSIS_EXE" && -z "$MAC_TAR" ]]; then
  echo "未找到任何更新产物（NSIS 或 app.tar.gz），请检查 $BUNDLE_DIR" >&2
  exit 1
fi

echo "==> 4/6 生成 latest.json（双源）"
GH_BASE="https://github.com/$GITHUB_REPO/releases/download/v$VERSION"
GITEE_BASE="https://gitee.com/$GITEE_REPO/releases/download/latest"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# 组装 latest.json：有 NSIS 则含 windows；macOS 产物存在（在 mac 上构建）才纳入
build_latest() { # $1 = 下载 base url
  local base="$1"
  local win_part="" mac_part=""
  if [[ -n "$NSIS_EXE" && -f "$NSIS_EXE.sig" ]]; then
    local nsis_name nsis_sig
    nsis_name="$(basename "$NSIS_EXE")"
    nsis_sig=$(cat "$NSIS_EXE.sig")
    win_part="\"windows-x86_64\": {\"signature\": \"$nsis_sig\", \"url\": \"$base/$nsis_name\"}"
  fi
  if [[ -n "$MAC_TAR" && -f "$MAC_TAR.sig" ]]; then
    local mac_name mac_sig
    mac_name="$(basename "$MAC_TAR")"
    mac_sig=$(cat "$MAC_TAR.sig")
    [[ -n "$win_part" ]] && win_part="$win_part,"
    win_part="$win_part\"darwin-aarch64\": {\"signature\": \"$mac_sig\", \"url\": \"$base/$mac_name\"}"
  fi
  cat <<EOF
{
  "version": "$VERSION",
  "notes": $(json_escape "${NOTES:-AgentHUD $VERSION}"),
  "pub_date": "$PUB_DATE",
  "platforms": { $win_part }
}
EOF
}

# JSON 字符串转义（反斜杠/引号/换行）
json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e ':a;N;$!ba;s/\n/\\n/g' | sed 's/^/"/;s/$/"/'
}

build_latest "$GH_BASE" > "$BUNDLE_DIR/latest.json"
build_latest "$GITEE_BASE" > "$BUNDLE_DIR/gitee-latest.json"
echo "    已生成 latest.json / gitee-latest.json"
if [[ "$UPLOAD" != "1" ]]; then
  echo "==> --no-upload：跳过上传。产物位置："
  ls -la "$BUNDLE_DIR/latest.json" "$BUNDLE_DIR/gitee-latest.json" || true
  exit 0
fi

# ---------- GitHub ----------
# 前置校验：无论 gh 还是 REST 路径，创建 release 都会在远程 main 当前 HEAD
# 自动打 tag；本地刚改的版本号若未提交推送，tag 就会落在旧版本号的提交上、
# CI 构建出错误版本（v1.7.0 踩坑），故上传前先核对远程 main 的版本号
git fetch origin main --quiet || { echo "错误：git fetch origin main 失败，无法校验远程版本号（检查网络与远程配置）" >&2; exit 1; }
REMOTE_VERSION=$(git show origin/main:src-tauri/tauri.conf.json | python3 -c "import json,sys;print(json.load(sys.stdin)['version'])") \
  || { echo "错误：读取远程 main 的 tauri.conf.json 版本号失败" >&2; exit 1; }
if [[ "$REMOTE_VERSION" != "$VERSION" ]]; then
  echo "错误：远程 main 上的版本号仍是 $REMOTE_VERSION，与本次发布版本 $VERSION 不一致。" >&2
  echo "无论 gh 还是 REST 路径，创建 release 都会在远程 main 当前 HEAD 自动打 tag；" >&2
  echo "版本号未提交推送时，tag 会落在旧版本号的提交上、CI 构建出错误版本（v1.7.0 踩坑）。" >&2
  echo "请先提交并推送版本号变更（package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml）再运行本脚本。" >&2
  exit 1
fi

echo "==> 5/6 上传 GitHub Release（tag v$VERSION，自动标记 latest）"
GH_FILES=("$BUNDLE_DIR/latest.json" "$BUNDLE_DIR/gitee-latest.json")
[[ -n "$NSIS_EXE" ]] && GH_FILES+=("$NSIS_EXE" "$NSIS_EXE.sig")
[[ -n "$MSI" ]] && GH_FILES+=("$MSI")
[[ -n "$MAC_TAR" ]] && GH_FILES+=("$MAC_TAR" "$MAC_TAR.sig")
# gh 优先；本机未装 gh 时用 GitHub PAT 走 REST API 兜底。API 请求走本机网络，
# curl 自动识别 https_proxy/HTTPS_PROXY 环境变量，需代理时运行前 export 即可，
# 脚本不硬编码
if command -v gh >/dev/null 2>&1; then
  gh release create "v$VERSION" "${GH_FILES[@]}" \
    --repo "$GITHUB_REPO" --title "AgentHUD v$VERSION" --notes "${NOTES:-AgentHUD v$VERSION}" \
    --latest
else
  # PAT 来源（按优先级）：GITHUB_TOKEN 环境变量 → git credential fill（读
  # Windows 凭据管理器/钥匙串里 git push 在用的 GitHub 凭据）
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    GH_PAT="$GITHUB_TOKEN"
  else
    # GIT_TERMINAL_PROMPT=0：无存储凭据时禁止交互式询问账号密码，直接失败
    CRED=$(printf 'protocol=https\nhost=github.com\n\n' | GIT_TERMINAL_PROMPT=0 git credential fill 2>/dev/null) || true
    GH_PAT=$(printf '%s\n' "$CRED" | sed -n 's/^password=//p')
  fi
  [[ -n "$GH_PAT" ]] || { echo "错误：未获取到 GitHub 凭据（GITHUB_TOKEN 或 git credential fill），请安装并登录 gh 后重试" >&2; exit 1; }
  command -v jq >/dev/null 2>&1 || { echo "错误：缺少 jq（无 gh 时用它转义 release 说明，见 publish_gitee.sh 同款用法）" >&2; exit 1; }
  # Git Bash 的 mingw64 curl 是原生 Windows 程序，读不了 MSYS 路径（curl 错误
  # 26，同 publish_gitee.sh upload() 的踩坑）；有 cygpath 时转 Windows 真实路径
  native_path() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi
  }

  # 查询 release 是否已存在：已存在直接报错（与 gh release create 拒绝重复
  # tag 的行为一致，避免重复发布），404 才走创建
  TAG_RESP=$(curl -sS --connect-timeout 20 --max-time 60 -w '\n%{http_code}' \
    -H "Authorization: Bearer $GH_PAT" \
    "https://api.github.com/repos/$GITHUB_REPO/releases/tags/v$VERSION") || true
  TAG_CODE="${TAG_RESP##*$'\n'}"
  if [[ "$TAG_CODE" == 2* ]]; then
    echo "错误：GitHub Release v$VERSION 已存在，请勿重复发布（gh release create 同样会拒绝）" >&2
    exit 1
  elif [[ "$TAG_CODE" != 404 ]]; then
    echo "错误：查询 GitHub Release 失败 HTTP ${TAG_CODE:-网络错误}（检查 PAT 权限）" >&2
    exit 1
  fi

  # 创建请求体走 UTF-8 JSON 文件 + jq 自检（publish_gitee.sh 的既有模式）：
  # body 必须先经 jq -Rs 转义（多行中文说明原样拼进 heredoc 会产生非法 JSON，
  # v1.1.0 发版踩坑），且 Windows 原生 curl 会把命令行中文按本地码页（GBK）
  # 转码，只有文件字节流 --data-binary @file 三端行为一致
  GH_JSON=$(mktemp "${TMPDIR:-/tmp}/zbar-gh-release.XXXXXX")
  BODY_JSON=$(printf '%s' "${NOTES:-AgentHUD v$VERSION}" | jq -Rs .)
  cat > "$GH_JSON" <<EOF
{"tag_name":"v${VERSION}","target_commitish":"main","name":"AgentHUD v${VERSION}","body":${BODY_JSON},"make_latest":"true"}
EOF
  jq -e . "$GH_JSON" >/dev/null
  CREATE_RESP=$(curl -sS --connect-timeout 20 --max-time 120 -w '\n%{http_code}' \
    -X POST "https://api.github.com/repos/$GITHUB_REPO/releases" \
    -H "Authorization: Bearer $GH_PAT" \
    -H "Content-Type: application/json" \
    --data-binary @"$(native_path "$GH_JSON")") || true
  rm -f "$GH_JSON"
  CREATE_CODE="${CREATE_RESP##*$'\n'}"
  CREATE_BODY="${CREATE_RESP%$'\n'*}"
  [[ "$CREATE_CODE" == 2* ]] || { echo "错误：创建 GitHub Release 失败 HTTP ${CREATE_CODE:-网络错误}：$(echo "$CREATE_BODY" | head -c 300)" >&2; exit 1; }
  RELEASE_ID=$(echo "$CREATE_BODY" | jq -r '.id // empty')
  RELEASE_URL=$(echo "$CREATE_BODY" | jq -r '.html_url // empty')
  [[ -n "$RELEASE_ID" ]] || { echo "错误：未解析到 GitHub Release id" >&2; exit 1; }
  echo "    release 已创建 (id=$RELEASE_ID)，上传 ${#GH_FILES[@]} 个文件"

  # 逐个上传 assets，每个确认 HTTP 2xx，失败报文件名退出
  for f in "${GH_FILES[@]}"; do
    fname="$(basename "$f")"
    UP_RESP=$(curl -sS --connect-timeout 20 --max-time 600 -w '\n%{http_code}' \
      -X POST "https://uploads.github.com/repos/$GITHUB_REPO/releases/$RELEASE_ID/assets?name=$fname" \
      -H "Authorization: Bearer $GH_PAT" \
      -H "Content-Type: application/octet-stream" \
      --data-binary @"$(native_path "$f")") || true
    UP_CODE="${UP_RESP##*$'\n'}"
    [[ "$UP_CODE" == 2* ]] || { echo "错误：上传 $fname 失败 HTTP ${UP_CODE:-网络错误}" >&2; exit 1; }
    echo "    已上传 $fname"
  done
  echo "    GitHub Release 完成：${RELEASE_URL:-https://github.com/$GITHUB_REPO/releases/tag/v$VERSION}"
fi

# ---------- Gitee（固定 tag latest，endpoint 用静态 URL）----------
# 背景：本机直连 Gitee 上传大附件会持续超时，Gitee 侧发布统一由
# scripts/publish_gitee.sh 本地直发（从 GitHub Release 拉产物后直传）；
# 需跳过本段时用 --no-gitee（Gitee 是最后一步，直接收尾）
if [[ "$SKIP_GITEE" == "1" ]]; then
  echo "==> 跳过 Gitee 上传（--no-gitee）——请随后运行 scripts/publish_gitee.sh $VERSION \"<说明>\""
  echo "    Gitee 侧产物位置（publish_gitee.sh 会从 GitHub Release 自动拉取）："
  ls -la "$BUNDLE_DIR/gitee-latest.json" || true
  exit 0
fi
echo "==> 6/6 上传 Gitee Release（固定 tag latest）"
: "${GITEE_TOKEN:?需要导出 GITEE_TOKEN（Gitee 设置→私人令牌，勾选 projects）}"
GITEE_API="https://gitee.com/api/v5/repos/$GITEE_REPO/releases"

# 删除旧 latest release（tag 保留复用；无则首次创建）
OLD_ID=$(curl -sf "$GITEE_API/tags/latest?access_token=$GITEE_TOKEN" | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2 || true)
if [[ -n "${OLD_ID:-}" ]]; then
  curl -sf -X DELETE "$GITEE_API/$OLD_ID?access_token=$GITEE_TOKEN" >/dev/null && echo "    已删除旧 latest release"
fi

# 附件先上传拿 file_path（不加 head -1：pipefail 下 head 提前关管道会让 grep 收
# SIGPIPE 返回非零被误判失败；响应本身单条，直接整条解析）
attach() { # $1 = 文件路径 $2 = 上传后的文件名（Gitee 以上传名为准）
  curl -sf -X POST "$GITEE_API/attach_files?access_token=$GITEE_TOKEN" \
    -F "file=@$1;filename=$2" | grep -o '"path":"[^"]*"' | cut -d'"' -f4
}
FILES=""
if [[ -n "$NSIS_EXE" ]]; then
  for f in "$NSIS_EXE" "$NSIS_EXE.sig"; do
    p=$(attach "$f" "$(basename "$f")"); FILES="$FILES$p,"
  done
  [[ -n "$MSI" ]] && { p=$(attach "$MSI" "$(basename "$MSI")"); FILES="$FILES$p,"; }
fi
[[ -n "$MAC_TAR" ]] && { p=$(attach "$MAC_TAR" "$(basename "$MAC_TAR")"); FILES="$FILES$p,"; }
p=$(attach "$BUNDLE_DIR/gitee-latest.json" "latest.json"); FILES="$FILES$p,"   # Gitee 侧文件名必须是 latest.json
FILES="${FILES%,}"

curl -sf -X POST "$GITEE_API" \
  -d "access_token=$GITEE_TOKEN" \
  -d "tag_name=latest" \
  -d "name=AgentHUD v$VERSION（应用内更新源）" \
  -d "body=${NOTES:-AgentHUD $VERSION 自动更新通道，保持最新版即可。}" \
  -d "files=$FILES" >/dev/null && echo "    Gitee release 已创建"

echo ""
echo "✅ 发布完成。请验证两个更新源可达："
echo "   https://github.com/$GITHUB_REPO/releases/latest/download/latest.json"
echo "   https://gitee.com/$GITEE_REPO/releases/download/latest/latest.json"
echo "别忘了提交代码并推送双仓库（git push / git push gitee main:master）。"
