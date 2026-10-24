/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{vue,ts}"],
  theme: {
    extend: {
      colors: {
        // 深色科技感：基调是冷黑紫，强调色是电光紫。
        brand: {
          50: "#f5f3ff",
          100: "#ede9fe",
          200: "#ddd6fe",
          300: "#c4b5fd",
          400: "#a78bfa",
          500: "#8b5cf6",
          600: "#7c3aed",
          700: "#6d28d9",
          800: "#5b21b6",
          900: "#4c1d95",
        },
        // ink 是文字层次：900 最亮（标题），300 最暗（辅助说明）
        ink: {
          900: "#f4f1ff",
          700: "#cdc7e3",
          500: "#928bab",
          400: "#6b6484",
          300: "#4a4460",
        },
        // surface 是背景层次：muted 是页面底，DEFAULT 是卡片，sunken 是凹陷区
        surface: {
          DEFAULT: "#131120",
          muted: "#0a0910",
          sunken: "#1c1930",
        },
        line: {
          DEFAULT: "rgba(255,255,255,0.07)",
          strong: "rgba(255,255,255,0.13)",
        },
      },
      borderRadius: {
        card: "16px",
        pill: "999px",
      },
      boxShadow: {
        // 深色下的层次靠"顶部高光 + 向外扩散的黑影"，而不是浅色那种灰影
        card: "inset 0 1px 0 rgba(255,255,255,0.05), 0 12px 32px -18px rgba(0,0,0,0.9)",
        "card-hover":
          "inset 0 1px 0 rgba(255,255,255,0.08), 0 20px 44px -20px rgba(0,0,0,0.95), 0 0 0 1px rgba(139,92,246,0.18)",
        pop: "0 24px 64px -20px rgba(0,0,0,0.95), 0 0 0 1px rgba(255,255,255,0.08)",
        glow: "0 0 0 1px rgba(139,92,246,0.35), 0 8px 32px -8px rgba(139,92,246,0.45)",
      },
      transitionTimingFunction: {
        smooth: "cubic-bezier(0.22, 1, 0.36, 1)",
      },
      keyframes: {
        "fade-up": {
          "0%": { opacity: "0", transform: "translateY(6px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
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
        shimmer: "shimmer 1.6s linear infinite",
        "pulse-soft": "pulse-soft 1.8s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
