import type { ReactNode, SVGProps } from "react";

/** 统计标签使用的品牌图标。图形内置为单色 SVG，不依赖运行时网络。 */
export type BrandIconName =
  | "zai"
  | "codex"
  | "claude"
  | "cursor"
  | "kimi"
  // 本地采集型（有独立品牌图形，走 BRAND_PATHS 单色 currentColor 渲染）
  | "dsh"
  // 凭证驱动的新 provider：品牌色圆底 + 白色首字母/简笔图形
  | "gemini"
  | "grok"
  | "qoder"
  | "opencodego"
  | "minimax"
  | "moonshot"
  | "deepseek"
  | "longcat"
  | "mimo"
  | "alibaba"
  | "alibabatoken"
  | "stepfun"
  | "doubao";

interface BrandIconProps extends SVGProps<SVGSVGElement> {
  brand: BrandIconName;
}

const BRAND_PATHS: Record<Exclude<BrandIconName, "zai" | "kimi" | "gemini" | "grok" | "qoder" | "opencodego" | "minimax" | "moonshot" | "deepseek" | "longcat" | "mimo" | "alibaba" | "alibabatoken" | "stepfun" | "doubao">, string> = {
  // OpenAI mark，Codex 使用 OpenAI 的品牌标识。
  codex:
    "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z",
  // Anthropic mark，Claude 使用 Anthropic 的品牌标识。
  claude: "M17.3041 3.541h-3.6718l6.696 16.918H24Zm-10.6082 0L0 20.459h3.7442l1.3693-3.5527h7.0052l1.3693 3.5528h3.7442L10.5363 3.5409Zm-.3712 10.2232 2.2914-5.9456 2.2914 5.9456Z",
  // Cursor mark。
  cursor:
    "M11.503.131 1.891 5.678a.84.84 0 0 0-.42.726v11.188c0 .3.162.575.42.724l9.609 5.55a1 1 0 0 0 .998 0l9.61-5.55a.84.84 0 0 0 .42-.724V6.404a.84.84 0 0 0-.42-.726L12.497.131a1.01 1.01 0 0 0-.996 0M2.657 6.338h18.55c.263 0 .43.287.297.515L12.23 22.918c-.062.107-.229.064-.229-.06V12.335a.59.59 0 0 0-.295-.51l-9.11-5.257c-.109-.063-.064-.23.061-.23",
  // Kimi Code 官方 mark（见下方 KIMI_CODE_PATH，与 zai 一样走独立分支：
  // 内部挖空依赖 nonzero 绕行方向 + fillRule，单色路径表放不下）。
  // DeepSeek Harness mark：鲸鱼（DeepSeek 鲸鱼意象 = 本地深潜采集）。
  // 身体（胶囊）+ 尾鳍（三角）+ 两粒喷气水花；眼睛是反向绕行的子路径
  // （nonzero 填充规则下挖空），单色 currentColor、viewBox 24×24。
  dsh: "M7.9 8.4h7.8a4.3 4.3 0 0 1 0 8.6H7.9a4.3 4.3 0 0 1 0-8.6ZM17.5 10 22.8 6.2v6.4ZM9.9 3.6a1.2 1.2 0 1 1-2.4 0 1.2 1.2 0 0 1 2.4 0ZM13.9 5.4a.9.9 0 1 1-1.8 0 .9.9 0 0 1 1.8 0ZM7.5 12.2a1.1 1.1 0 1 0 2.2 0 1.1 1.1 0 0 0-2.2 0Z",
};

/**
 * Kimi Code 官方 mark（Kimi Code CLI，https://github.com/MoonshotAI/kimi-code）。
 *
 * 形态：吉祥物头部（圆，下半压在控制条之后）+ 右上角圆点 + 深色控制条，
 * 内部挖空两个眼睛、控制条上的「>」提示符与光标短横（对应官方 App 图标里
 * 那条浅蓝短横）。
 *
 * 几何不是手绘近似，而是从官方 Windows 安装目录的
 * <Kimi Code 安装目录>/resources/build/icon.ico（256px 帧）逐元素量出后按
 * 比例换算到 24×24（实测与官方 mark 的 IoU 0.95，残差均为 1px 边缘抗锯齿）：
 * 头部圆心/半径、控制条圆角、两眼的胶囊尺寸与「右眼略高」这一原始不对称、
 * 圆点与「>」的笔画宽度都取实测值，未做对称化「美化」。
 *
 * 子路径绕行方向：头/控制条/圆点/尖角为顺时针（实心），两眼睛、「>」两臂、
 * 光标短横为逆时针（挖空）。nonzero 填充规则下逆时针子路径在任何背景上都
 * 真正透空，故不依赖背景色（与 dsh 鲸鱼眼睛同一手法），深色主题同样正确
 * ——这也是它不能像 codex/claude 那样只放一条单色 path 的原因。
 */
const KIMI_CODE_PATH =
  // 头部：整圆（下半被控制条遮住；同为顺时针实心，重叠不会互相抵消）
  "M1.48 11.88A9.62 9.62 0 1 1 20.72 11.88A9.62 9.62 0 1 1 1.48 11.88Z" +
  // 控制条：圆角矩形
  "M1.78 14.58H22.22A0.72 0.72 0 0 1 22.94 15.31V20.96A0.72 0.72 0 0 1 22.22 21.68" +
  "H1.78A0.72 0.72 0 0 1 1.06 20.96V15.31A0.72 0.72 0 0 1 1.78 14.58Z" +
  // 右上角圆点 + 左下尖角（两块都顺时针，合成水滴状外轮廓）
  "M19.33 3.83A1.62 1.62 0 1 1 22.58 3.83A1.62 1.62 0 1 1 19.33 3.83Z" +
  "M19.33 3.46L20.72 5.45L19.33 5.45Z" +
  // 左眼（胶囊；实测 x 110..131、y 84.5..121.5）
  "M9.9 8.27L9.9 10.2A1.26 1.26 0 0 0 12.42 10.2L12.42 8.27A1.26 1.26 0 0 0 9.9 8.27Z" +
  // 右眼（比左眼略高，为官方原始比例）
  "M14.92 7.58L14.92 9.45A1.17 1.17 0 0 0 17.26 9.45L17.26 7.58A1.17 1.17 0 0 0 14.92 7.58Z" +
  // 「>」提示符：两条圆头粗线段（逆时针 → 在控制条上挖空）
  "M2.61 17.34L4.53 18.54A0.48 0.48 0 0 0 5.04 17.72L3.12 16.52A0.48 0.48 0 0 0 2.61 17.34Z" +
  "M3.12 19.74L5.04 18.54A0.48 0.48 0 0 0 4.53 17.72L2.61 18.93A0.48 0.48 0 0 0 3.12 19.74Z" +
  // 光标短横
  "M7.01 20.23L8.93 20.23A0.48 0.48 0 0 0 8.93 19.27L7.01 19.27A0.48 0.48 0 0 0 7.01 20.23Z";


// ============================================================
// 凭证驱动的新 provider 图标：品牌色圆底 + 白色首字母/简笔图形。
// 保持与既有图标近似的视觉重量（无描边、纯色块），不引入外部资源。
// ============================================================

/** 白色首字母字形（圆底内居中） */
function letterTile(
  letter: string,
  fontSize = 12.5,
  fg = "#fff"
): ReactNode {
  return (
    <text
      x="12"
      y="12.6"
      textAnchor="middle"
      dominantBaseline="middle"
      fill={fg}
      fontSize={fontSize}
      fontWeight="700"
      fontFamily="system-ui, -apple-system, 'Segoe UI', sans-serif"
    >
      {letter}
    </text>
  );
}

/** 新 provider 的圆底色块定义（bg=底色；icon=白色图形）。
 *  Exclude 与 BRAND_PATHS 互补：进 BRAND_PATHS 的（zai/dsh）必须在此排除。 */
const CREDENTIAL_AGENT_TILES: Record<
  Exclude<BrandIconName, "zai" | "dsh" | "codex" | "claude" | "cursor" | "kimi">,
  { bg: string; icon: ReactNode }
> = {
  // Gemini：四角星（Sparkle 品牌意象）
  gemini: {
    bg: "#4285F4",
    icon: (
      <path
        fill="#fff"
        d="M12 4.2 13.9 10.1 19.8 12 13.9 13.9 12 19.8 10.1 13.9 4.2 12 10.1 10.1Z"
      />
    ),
  },
  grok: { bg: "#1d9bf0", icon: letterTile("G") },
  qoder: { bg: "#6c5ce7", icon: letterTile("Q") },
  // OpenCode：终端提示符 >_ 意象
  opencodego: {
    bg: "#f97316",
    icon: (
      <text
        x="12"
        y="12.8"
        textAnchor="middle"
        dominantBaseline="middle"
        fill="#fff"
        fontSize="9.5"
        fontWeight="700"
        fontFamily="ui-monospace, 'Cascadia Mono', Consolas, monospace"
      >
        &gt;_
      </text>
    ),
  },
  minimax: { bg: "#00b96b", icon: letterTile("M") },
  // Kimi（月之暗面开放平台，provider id 仍为 moonshot）：月光月牙。
  // 与 Kimi Code 的吉祥物 mark 区分开——月牙是月之暗面的品牌意象。
  moonshot: {
    bg: "#4540d6",
    icon: (
      <path
        fill="#fff"
        d="M19.3 13.6A7.6 7.6 0 1 1 11.4 5a6 6 0 0 0 7.9 8.6Z"
      />
    ),
  },
  deepseek: { bg: "#4D6BFE", icon: letterTile("D") },
  // LongCat：亮黄底配深色字（白字对比不足）
  longcat: { bg: "#ffd100", icon: letterTile("L", 12.5, "#6b5600") },
  mimo: { bg: "#ff6900", icon: letterTile("m", 13) },
  alibaba: { bg: "#ff6a00", icon: letterTile("Q") },
  // 百炼 Token 包：深色「百」字与 alibaba 的白「Q」拉开差异（同系品牌橙底）
  alibabatoken: { bg: "#ff9240", icon: letterTile("百", 11, "#5c2c00") },
  stepfun: { bg: "#3b82f6", icon: letterTile("S") },
  doubao: { bg: "#00d4aa", icon: letterTile("火", 11) },
};

export function BrandIcon({ brand, ...props }: BrandIconProps) {
  if (brand === "zai") {
    return (
      <svg
        {...props}
        viewBox="0 0 30 30"
        fill="none"
        aria-hidden="true"
        focusable="false"
      >
        <path
          fill="currentColor"
          d="M24.51 28.51H5.49c-2.21 0-4-1.79-4-4V5.49c0-2.21 1.79-4 4-4h19.03c2.21 0 4 1.79 4 4v19.03c-.01 2.2-1.8 3.99-4.01 3.99Z"
        />
        <path
          fill="#fff"
          d="m15.47 7.1-1.3 1.85c-.2.29-.54.47-.9.47h-7.1V7.09c-.01.01 9.3.01 9.3.01Zm8.83 0-11.16 15.81H5.7L16.86 7.1h7.44Zm-9.77 15.81 1.31-1.86c.2-.29.54-.47.9-.47h7.09v2.33h-9.3Z"
        />
      </svg>
    );
  }

  // Kimi Code 走独立分支：挖空依赖 nonzero 绕行方向，需显式 fillRule
  if (brand === "kimi") {
    return (
      <svg
        {...props}
        viewBox="0 0 24 24"
        fill="currentColor"
        aria-hidden="true"
        focusable="false"
      >
        <path fillRule="nonzero" d={KIMI_CODE_PATH} />
      </svg>
    );
  }

  const tile = (CREDENTIAL_AGENT_TILES as Record<string, { bg: string; icon: ReactNode } | undefined>)[
    brand
  ];
  if (tile) {
    return (
      <svg
        {...props}
        viewBox="0 0 24 24"
        aria-hidden="true"
        focusable="false"
      >
        <circle cx="12" cy="12" r="12" fill={tile.bg} />
        {tile.icon}
      </svg>
    );
  }

  return (
    <svg
      {...props}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
    >
      <path d={BRAND_PATHS[brand as keyof typeof BRAND_PATHS]} />
    </svg>
  );
}
