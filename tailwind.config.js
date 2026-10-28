/**
 * 语义色板全部走 CSS 变量（见 src/style.css）。
 *
 * 这样深色 / 浅色只差一份变量表，组件里继续写 `bg-surface`、`text-ink-500`
 * 就行，不用到处 `dark:` 前缀。变量存的是 RGB 通道，所以 `bg-brand-500/16`
 * 这类透明度写法照样生效。
 */
const ch = (name) => `rgb(var(--${name}) / <alpha-value>)`;

const brand = Object.fromEntries(
  [50, 100, 200, 300, 400, 500, 600, 700, 800, 900].map((step) => [
    step,
    ch(`brand-${step}`),
  ]),
);

const ink = Object.fromEntries(
  [900, 800, 700, 600, 500, 400, 300].map((step) => [step, ch(`ink-${step}`)]),
);

/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{vue,ts}"],
  theme: {
    extend: {
      colors: {
        brand,
        // ink 是文字层次：900 最重（标题），300 最轻（辅助说明）
        ink,
        // surface 是背景层次：muted 是页面底，DEFAULT 是卡片，sunken 是凹陷区
        surface: {
          DEFAULT: ch("surface"),
          muted: ch("surface-muted"),
          sunken: ch("surface-sunken"),
        },
        // line 是描边（带透明度，所以直接给完整 rgba）
        line: {
          DEFAULT: "var(--line)",
          strong: "var(--line-strong)",
        },
        // veil 是"叠在表面上的那一层"：深色下是白、浅色下是墨，
        // 用来替代硬编码的 bg-white/[0.04]、border-white/[0.09]
        veil: ch("veil"),
      },
      borderRadius: {
        card: "16px",
        pill: "999px",
      },
      boxShadow: {
        // 深色靠"顶部高光 + 扩散黑影"，浅色靠柔和灰影，具体值在变量里切换
        card: "var(--shadow-card)",
        "card-hover": "var(--shadow-card-hover)",
        pop: "var(--shadow-pop)",
        glow: "0 0 0 1px rgba(139,92,246,0.35), 0 8px 32px -8px rgba(139,92,246,0.45)",
      },
      transitionTimingFunction: {
        smooth: "cubic-bezier(0.22, 1, 0.36, 1)",
        // Q 弹：末段过冲一点点再回落，按钮 / 开关 / 卡片抬起用
        spring: "cubic-bezier(0.34, 1.56, 0.64, 1)",
        // 收得住的回弹：过冲更小，用于小控件
        snap: "cubic-bezier(0.22, 1.24, 0.36, 1)",
      },
      keyframes: {
        "fade-up": {
          "0%": { opacity: "0", transform: "translateY(6px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
        // 弹窗 / 菜单出现：先略缩再弹回
        pop: {
          "0%": { opacity: "0", transform: "scale(0.94)" },
          "65%": { opacity: "1", transform: "scale(1.015)" },
          "100%": { opacity: "1", transform: "scale(1)" },
        },
        shimmer: {
          "0%": { backgroundPosition: "-200% 0" },
          "100%": { backgroundPosition: "200% 0" },
        },
        "pulse-soft": {
          "0%, 100%": { opacity: "0.45" },
          "50%": { opacity: "0.9" },
        },
      },
      animation: {
        "fade-up": "fade-up 0.24s cubic-bezier(0.22,1,0.36,1) both",
        pop: "pop 0.22s cubic-bezier(0.34,1.56,0.64,1) both",
        shimmer: "shimmer 1.6s linear infinite",
        "pulse-soft": "pulse-soft 1.8s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
