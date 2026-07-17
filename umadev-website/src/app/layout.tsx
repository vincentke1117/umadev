import type { Metadata } from "next";
import { JetBrains_Mono, Manrope, Space_Grotesk } from "next/font/google";
import "./globals.css";
import { asset } from "./content";

const manrope = Manrope({
  variable: "--font-manrope",
  subsets: ["latin"],
  display: "swap",
});

const spaceGrotesk = Space_Grotesk({
  variable: "--font-space",
  subsets: ["latin"],
  display: "swap",
});

const jetBrainsMono = JetBrains_Mono({
  variable: "--font-mono",
  subsets: ["latin"],
  display: "swap",
});

export const metadata: Metadata = {
  metadataBase: new URL(process.env.NEXT_PUBLIC_SITE_URL ?? "https://umadev.goder.ai"),
  title: "UmaDev — 一个模拟真实开发团队、驱动你的底座干活的 Agent",
  description:
    "UmaDev 深度适配五个一等本机编码底座；Claude Code、Codex、OpenCode 采用厂商专属协议，Grok Build 与 Kimi Code 采用厂商官方 ACP v1 接口和隔离配置。",
  icons: {
    icon: asset("/assets/umadev-icon.png"),
    apple: asset("/assets/umadev-icon.png"),
  },
  openGraph: {
    title: "UmaDev — One agent. A whole development team at work.",
    description:
      "Deeply integrate one of five first-class local coding bases: Claude Code, Codex, OpenCode, Grok Build, or Kimi Code.",
    type: "website",
    images: [{ url: asset("/assets/wide-1.png"), width: 1672, height: 941 }],
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="zh-CN"
      className={`${manrope.variable} ${spaceGrotesk.variable} ${jetBrainsMono.variable}`}
      suppressHydrationWarning
    >
      <body suppressHydrationWarning>{children}</body>
    </html>
  );
}
