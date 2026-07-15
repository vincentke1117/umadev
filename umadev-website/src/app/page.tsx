"use client";

import Image from "next/image";
import React, { useEffect, useRef, useState } from "react";
import { asset, docs, gallery, i18n, releases, type DocBlock, type Lang, type View } from "./content";
import styles from "./page.module.css";

const githubUrl = "https://github.com/umacloud/umadev";
type DocItem = { id: string; title: string; blocks: readonly DocBlock[] };
type DocCategory = { cat: string; items: readonly DocItem[] };

const CHARS = "010101010101ABCDEF0123456789X%#$@";

export function ScrambledHoverText({ text, className }: { text: string; className?: string }) {
  const [displayText, setDisplayText] = useState(text);
  const [prevText, setPrevText] = useState(text);

  if (text !== prevText) {
    setPrevText(text);
    setDisplayText(text);
  }

  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const handleMouseEnter = () => {
    let iteration = 0;
    if (intervalRef.current) clearInterval(intervalRef.current);

    intervalRef.current = setInterval(() => {
      setDisplayText(
        text
          .split("")
          .map((char, index) => {
            if (char === " ") return " ";
            if (index < iteration) return text[index];
            return CHARS[Math.floor(Math.random() * CHARS.length)];
          })
          .join("")
      );

      iteration += 1 / 2;
      if (iteration >= text.length) {
        setDisplayText(text);
        if (intervalRef.current) clearInterval(intervalRef.current);
      }
    }, 25);
  };

  const handleMouseLeave = () => {
    if (intervalRef.current) clearInterval(intervalRef.current);
    setDisplayText(text);
  };

  useEffect(() => {
    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, []);

  return (
    <span
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
      className={className}
      style={{ position: "relative", display: "inline-block" }}
    >
      <span style={{ visibility: "hidden" }} aria-hidden="true">{text}</span>
      <span style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", whiteSpace: "nowrap" }}>
        {displayText}
      </span>
    </span>
  );
}


export function FloatingDiagnosticTerminal({
  setLang,
  setView,
  setHeroSlideIndex,
  setMode,
  setStageIndex,
}: {
  setLang: (l: Lang) => void;
  setView: (v: View) => void;
  setHeroSlideIndex: (s: number | ((prev: number) => number)) => void;
  setMode: (m: string) => void;
  setStageIndex: (i: number) => void;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const [logs, setLogs] = useState<string[]>([
    "UMADEV CORE DIAGNOSTICS v1.0.6",
    "Initializing MCP channels... OK",
    "Status: ONLINE [SECURE_MODE]",
    "Type /help for diagnostic commands."
  ]);
  const [inputValue, setInputValue] = useState("");
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (bodyRef.current) {
      bodyRef.current.scrollTop = bodyRef.current.scrollHeight;
    }
  }, [logs, isOpen]);

  const handleCommand = (cmdStr: string) => {
    const raw = cmdStr.trim();
    if (!raw) return;

    setLogs((prev) => [...prev, `umadev_hud > ${raw}`]);
    const tokens = raw.split(/\s+/);
    const cmd = tokens[0].toLowerCase();
    const arg = tokens[1]?.toLowerCase();

    setTimeout(() => {
      if (cmd === "/help" || cmd === "help") {
        setLogs((prev) => [
          ...prev,
          "--- Active HUD Controller Commands ---",
          "  /lang <zh|en>     - Switch language",
          "  /slide <1-3|next> - Switch Hero visual slides",
          "  /mode <id>        - Switch base (claude-code | codex | opencode)",
          "  /stage <1-3>      - Switch pipeline stage",
          "  /view <home|docs> - Navigate page views",
          "  /stats            - Print system & browser diagnostics",
          "  /scan             - Run code & secret checks",
          "  /governance       - Verify governance checklists",
          "  /clear            - Clear console screen"
        ]);
      } else if (cmd === "/clear" || cmd === "clear") {
        setLogs([]);
      } else if (cmd === "/lang") {
        if (arg === "en") {
          setLang("en");
          setLogs((prev) => [...prev, "System: Language set to ENGLISH."]);
        } else if (arg === "zh") {
          setLang("zh");
          setLogs((prev) => [...prev, "系统：语言切换为【中文】。"]);
        } else {
          setLogs((prev) => [...prev, "Usage: /lang <zh|en>"]);
        }
      } else if (cmd === "/slide") {
        const val = parseInt(arg, 10);
        if (arg === "next") {
          setHeroSlideIndex((prev) => (prev + 1) % 3);
          setLogs((prev) => [...prev, "System: Rotated to next hero slide."]);
        } else if (val >= 1 && val <= 3) {
          setHeroSlideIndex(val - 1);
          setLogs((prev) => [...prev, `System: Loaded hero slide ${val}.`]);
        } else {
          setLogs((prev) => [...prev, "Usage: /slide <1-3|next>"]);
        }
      } else if (cmd === "/mode") {
        const valid = ["claude-code", "codex", "opencode"];
        if (valid.includes(arg)) {
          setMode(arg);
          setLogs((prev) => [...prev, `System: Base switched to ${arg}.`]);
        } else {
          setLogs((prev) => [...prev, "Usage: /mode <claude-code|codex|opencode>"]);
        }
      } else if (cmd === "/stage") {
        const val = parseInt(arg, 10);
        if (val >= 1 && val <= 3) {
          setStageIndex(val - 1);
          setLogs((prev) => [...prev, `System: Pipeline stage set to ${val}.`]);
        } else {
          setLogs((prev) => [...prev, "Usage: /stage <1-3>"]);
        }
      } else if (cmd === "/view") {
        if (arg === "home" || arg === "docs") {
          setView(arg);
          setLogs((prev) => [...prev, `System: Navigating to ${arg.toUpperCase()} view.`]);
        } else {
          setLogs((prev) => [...prev, "Usage: /view <home|docs>"]);
        }
      } else if (cmd === "/stats" || cmd === "stats") {
        const res = typeof window !== "undefined" ? `${window.screen.width}x${window.screen.height}` : "Unknown";
        const ua = typeof navigator !== "undefined" ? navigator.userAgent.split(" ").slice(-2).join(" ") : "Unknown";
        const conn = typeof navigator !== "undefined" && (navigator as unknown as { connection?: { rtt?: number; downlink?: number } }).connection
          ? `RTT: ${(navigator as unknown as { connection: { rtt: number } }).connection.rtt}ms, Speed: ${(navigator as unknown as { connection: { downlink: number } }).connection.downlink}Mbps`
          : "N/A";
        setLogs((prev) => [
          ...prev,
          "--- Browser & System Diagnostics ---",
          `  Resolution  : ${res}`,
          `  UserAgent   : ${ua}`,
          `  Network     : ${conn}`,
          `  App Status  : ONLINE (Ready)`,
          `  Engine      : Next.js 16.2.9 (Turbopack)`
        ]);
      } else if (cmd === "/scan" || cmd === "scan") {
        setLogs((prev) => [
          ...prev,
          "Scanning workspace... [■■■■■■■■■■] 100%",
          "  -> Found 9 modules in Rust workspace",
          "  -> 0 hardcoded secrets detected",
          "  -> 0 clippy warnings",
          "  -> Quality Gate: 94% PASS"
        ]);
      } else if (cmd === "/governance" || cmd === "governance") {
        setLogs((prev) => [
          ...prev,
          "Checking 113 governance rules...",
          "  [RULE_01] No raw unwraps - PASS",
          "  [RULE_02] Secure MCP transport - PASS",
          "  [RULE_03] No hardcoded styles - PASS",
          "Governance Status: SECURE / FAILS OPEN"
        ]);
      } else if (cmd === "/deploy" || cmd === "deploy") {
        setLogs((prev) => [
          ...prev,
          "Triggering local deploy simulator...",
          "  Running subprocess: next build...",
          "  Deploying static bundle to GitHub Pages...",
          "  Success! Domain: umadev.dev/preview"
        ]);
      } else if (cmd === "/proof" || cmd === "proof") {
        setLogs((prev) => [
          ...prev,
          "Compiling Proof Pack...",
          "  Archiving .umadev/audit/session.jsonl...",
          "  Generating scorecard-v1.0.6.html...",
          "  Created delivery: release/proof-pack.zip"
        ]);
      } else {
        setLogs((prev) => [
          ...prev,
          `Error: Command "${raw}" not recognized. Type /help for assistance.`
        ]);
      }
    }, 100);

    setInputValue("");
  };

  return (
    <>
      {!isOpen && (
        <button
          className={styles.hudFloatBtn}
          onClick={() => setIsOpen(true)}
          title="Open system diagnostics console"
        >
          <span className={styles.hudPulseLight} />
          <span>SYS_OK: 94%</span>
        </button>
      )}

      {isOpen && (
        <div className={styles.hudOverlayTerm}>
          <div className={styles.hudTermHeader}>
            <span>SYS DIAGNOSTICS</span>
            <button onClick={() => setIsOpen(false)}>×</button>
          </div>
          <div className={styles.hudTermBody} ref={bodyRef}>
            {logs.map((log, i) => (
              <div key={i} className={styles.hudTermLine}>
                {log}
              </div>
            ))}
          </div>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              handleCommand(inputValue);
            }}
            className={styles.hudTermForm}
          >
            <span>&gt;</span>
            <input
              type="text"
              value={inputValue}
              onChange={(e) => setInputValue(e.target.value)}
              placeholder="Type /help..."
              autoFocus
            />
          </form>
        </div>
      )}
    </>
  );
}
export default function Home({ initialView }: { initialView?: View } = {}) {
  const [lang, setLang] = useState<Lang>("zh");
  const [view, setView] = useState<View>(initialView ?? "home");
  const [wechatOpen, setWechatOpen] = useState(false);
  const [activeStageIdx, setActiveStageIdx] = useState(0);
  const [autoplay, setAutoplay] = useState(true);

  useEffect(() => {
    if (!autoplay) return;
    const interval = setInterval(() => {
      setActiveStageIdx((prev) => (prev + 1) % 10);
    }, 4000);
    return () => clearInterval(interval);
  }, [autoplay]);

  const handleTiltMove = (e: React.MouseEvent<HTMLElement>) => {
    const c = e.currentTarget;
    const r = c.getBoundingClientRect();
    const px = (e.clientX - r.left) / r.width - 0.5;
    const py = (e.clientY - r.top) / r.height - 0.5;
    c.style.setProperty("--tilt-x", `${-py * 5}deg`);
    c.style.setProperty("--tilt-y", `${px * 6}deg`);
    c.style.setProperty("--spot-x", `${(px + 0.5) * 100}%`);
    c.style.setProperty("--spot-y", `${(py + 0.5) * 100}%`);
    c.style.transform = `perspective(700px) rotateX(${-py * 8}deg) rotateY(${px * 8}deg) translateZ(10px) translateY(-5px)`;
    c.style.transition = "transform 0.1s cubic-bezier(0.25, 0.8, 0.25, 1)";
  };
  const handleTiltLeave = (e: React.MouseEvent<HTMLElement>) => {
    const c = e.currentTarget;
    c.style.setProperty("--tilt-x", "0deg");
    c.style.setProperty("--tilt-y", "0deg");
    c.style.setProperty("--spot-x", "50%");
    c.style.setProperty("--spot-y", "50%");
    c.style.transform = "perspective(700px) rotateX(0) rotateY(0) translateZ(0) translateY(0)";
    c.style.transition = "transform 0.4s cubic-bezier(0.25, 0.8, 0.25, 1)";
  };

  const handleMagnetMove = (e: React.MouseEvent<HTMLElement>) => {
    const btn = e.currentTarget;
    const r = btn.getBoundingClientRect();
    const mx = e.clientX - r.left - r.width / 2;
    const my = e.clientY - r.top - r.height / 2;
    btn.style.transform = `translate(${mx * 0.35}px, ${my * 0.4}px) scale(1.04)`;
    btn.style.transition = "transform 0.1s cubic-bezier(.16,1,.3,1)";
  };
  const handleMagnetLeave = (e: React.MouseEvent<HTMLElement>) => {
    const btn = e.currentTarget;
    btn.style.transform = "translate(0,0) scale(1)";
    btn.style.transition = "transform 0.4s cubic-bezier(.16,1,.3,1)";
  };

  // Auto language detection based on browser locale
  useEffect(() => {
    /* eslint-disable react-hooks/set-state-in-effect */
    const savedLang = localStorage.getItem("umadev_lang") as Lang | null;
    if (savedLang === "zh" || savedLang === "en") {
      setLang(savedLang);
      return;
    }
    const browserLang = navigator.language || (navigator as unknown as { userLanguage?: string }).userLanguage || "";
    if (browserLang.toLowerCase().includes("zh")) {
      setLang("zh");
    } else {
      setLang("en");
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, []);

  // Synchronize language selection to localStorage
  useEffect(() => {
    localStorage.setItem("umadev_lang", lang);
  }, [lang]);

  // Sync initial view from URL pathname and handle browser back/forward buttons
  useEffect(() => {
    /* eslint-disable react-hooks/set-state-in-effect */
    if (typeof window !== "undefined") {
      const pathToView = (path: string): View => {
        const cleanPath = path.replace(process.env.NEXT_PUBLIC_BASE_PATH ?? "", "").replace(/\/$/, "");
        if (cleanPath === "/docs" || cleanPath === "docs") return "docs";
        if (cleanPath === "/gallery" || cleanPath === "gallery") return "gallery";
        if (cleanPath === "/changelog" || cleanPath === "changelog") return "changelog";
        if (cleanPath === "/contributors" || cleanPath === "contributors") return "contributors";
        return "home";
      };

      const currentView = pathToView(window.location.pathname);
      if (!initialView && currentView !== view) {
        setView(currentView);
      }

      const handlePopState = () => {
        setView(pathToView(window.location.pathname));
      };
      window.addEventListener("popstate", handlePopState);
      return () => window.removeEventListener("popstate", handlePopState);
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, [initialView, view]);

  const scrollProgressRef = useRef<HTMLDivElement | null>(null);
  const [docId, setDocId] = useState("quickstart");
  const [lightbox, setLightbox] = useState<number | null>(null);
  const [copied, setCopied] = useState(false);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Spotlight, tilt, magnetic, and scroll reveal animations from Umadevweb
  useEffect(() => {
    // 1. Magnetic Buttons
    const magnets = document.querySelectorAll(`.${styles.umaMagnet}`);
    const magnetCleanups: (() => void)[] = [];
    magnets.forEach((node) => {
      const btn = node as HTMLElement;
      const handleMove = (e: MouseEvent) => {
        const r = btn.getBoundingClientRect();
        const mx = e.clientX - r.left - r.width / 2;
        const my = e.clientY - r.top - r.height / 2;
        btn.style.transform = `translate(${mx * 0.28}px, ${my * 0.32}px)`;
      };
      const handleLeave = () => {
        btn.style.transform = "translate(0,0)";
      };
      btn.addEventListener("mousemove", handleMove);
      btn.addEventListener("mouseleave", handleLeave);
      btn.style.transition = "transform .18s cubic-bezier(.16,1,.3,1)";
      magnetCleanups.push(() => {
        btn.removeEventListener("mousemove", handleMove);
        btn.removeEventListener("mouseleave", handleLeave);
      });
    });

    // 2. Tilt Cards
    const tilts = document.querySelectorAll(`.${styles.tilt}`);
    const tiltCleanups: (() => void)[] = [];
    tilts.forEach((node) => {
      const c = node as HTMLElement;
      const handleMove = (e: MouseEvent) => {
        const r = c.getBoundingClientRect();
        const px = (e.clientX - r.left) / r.width - 0.5;
        const py = (e.clientY - r.top) / r.height - 0.5;
        c.style.transform = `perspective(700px) rotateX(${-py * 7}deg) rotateY(${px * 7}deg) translateZ(6px)`;
      };
      const handleLeave = () => {
        c.style.transform = "perspective(700px) rotateX(0) rotateY(0)";
      };
      c.addEventListener("mousemove", handleMove);
      c.addEventListener("mouseleave", handleLeave);
      c.style.transition = "transform .18s ease";
      tiltCleanups.push(() => {
        c.removeEventListener("mousemove", handleMove);
        c.removeEventListener("mouseleave", handleLeave);
      });
    });

    // 3. Scroll Reveal
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            entry.target.classList.add(styles.in);
            observer.unobserve(entry.target);
          }
        });
      },
      { threshold: 0.12 }
    );
    const reveals = document.querySelectorAll(`.${styles.reveal}`);
    reveals.forEach((r) => observer.observe(r));

    // 4. Count up animation
    const countObserver = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          const el = entry.target as HTMLElement;
          const target = +(el.getAttribute("data-count") || "0");
          let t0: number | null = null;
          const step = (ts: number) => {
            if (!t0) t0 = ts;
            const progress = Math.min((ts - t0) / 1400, 1);
            const eased = 1 - Math.pow(1 - progress, 3);
            el.textContent = String(Math.round(target * eased));
            if (progress < 1) requestAnimationFrame(step);
          };
          requestAnimationFrame(step);
          countObserver.unobserve(el);
        });
      },
      { threshold: 0.5 }
    );
    const counters = document.querySelectorAll(`[data-count]`);
    counters.forEach((el) => countObserver.observe(el));

    return () => {
      magnetCleanups.forEach((c) => c());
      tiltCleanups.forEach((c) => c());
      observer.disconnect();
      countObserver.disconnect();
    };
  }, [view]);

  useEffect(() => {
    let raf = 0;
    const updateScrollProgress = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        const max = document.documentElement.scrollHeight - window.innerHeight;
        const progress = max > 0 ? Math.min(window.scrollY / max, 1) : 0;
        scrollProgressRef.current?.style.setProperty("--scroll-progress", String(progress));
        raf = 0;
      });
    };
    updateScrollProgress();
    window.addEventListener("scroll", updateScrollProgress, { passive: true });
    window.addEventListener("resize", updateScrollProgress);
    return () => {
      window.removeEventListener("scroll", updateScrollProgress);
      window.removeEventListener("resize", updateScrollProgress);
      cancelAnimationFrame(raf);
    };
  }, [view]);

  const t = i18n[lang];
  const docCats = docs[lang] as readonly DocCategory[];
  const activeDoc =
    docCats.flatMap((cat) => cat.items).find((item) => item.id === docId) ??
    docCats[0].items[0];

  // Dynamically update document title and description based on language and view
  useEffect(() => {
    let title = "";
    if (view === "home") {
      title = lang === "zh"
        ? "UmaDev - 一个模拟真实开发团队工作的 Agent,指挥你已经在用的 Claude Code / Codex / OpenCode 干活"
        : "UmaDev — a coding agent that works like a real dev team, commanding the Claude Code / Codex / OpenCode you already use";
    } else if (view === "docs") {
      title = lang === "zh"
        ? "文档中心 | UmaDev - 一个模拟真实开发团队工作的 Agent,指挥你已经在用的 Claude Code / Codex / OpenCode 干活"
        : "Documentation | UmaDev — a coding agent that works like a real dev team, commanding the Claude Code / Codex / OpenCode you already use";
    } else if (view === "gallery") {
      title = lang === "zh"
        ? "形象相册 | UmaDev - 一个模拟真实开发团队工作的 Agent,指挥你已经在用的 Claude Code / Codex / OpenCode 干活"
        : "Mascot Gallery | UmaDev — a coding agent that works like a real dev team, commanding the Claude Code / Codex / OpenCode you already use";
    } else if (view === "changelog") {
      title = lang === "zh"
        ? "更新日志 | UmaDev - 一个模拟真实开发团队工作的 Agent,指挥你已经在用的 Claude Code / Codex / OpenCode 干活"
        : "Changelog | UmaDev — a coding agent that works like a real dev team, commanding the Claude Code / Codex / OpenCode you already use";
    } else if (view === "contributors") {
      title = lang === "zh"
        ? "特别贡献荣誉殿堂 | UmaDev - 一个模拟真实开发团队工作的 Agent,指挥你已经在用的 Claude Code / Codex / OpenCode 干活"
        : "Special Contributors | UmaDev — a coding agent that works like a real dev team, commanding the Claude Code / Codex / OpenCode you already use";
    }

    document.title = title;

    const descMeta = document.querySelector('meta[name="description"]');
    if (descMeta) {
      descMeta.setAttribute(
        "content",
        lang === "zh"
          ? "UmaDev 是一个模拟真实开发团队来工作的 Coding Agent：产品经理、架构师、设计师、前端、后端、QA、安全、DevOps 八个角色分工协作，借你已登录的 Claude Code / Codex / OpenCode 大脑，把一句需求做成能上线的商业级应用。"
          : "UmaDev is a coding agent that works like a real dev team — eight specialists collaborating to turn one idea into a shippable, commercial-grade app, borrowing your logged-in Claude Code / Codex / OpenCode brain."
      );
    }
  }, [lang, view]);

  // Docs: scroll-spy — highlight the sidebar entry for the section near the top
  // of the viewport as the reader scrolls (the big-docs pattern).
  useEffect(() => {
    if (view !== "docs") return;
    const sections = Array.from(
      document.querySelectorAll<HTMLElement>("[data-doc-section]"),
    );
    if (sections.length === 0) return;
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible[0]) setDocId(visible[0].target.id);
      },
      { rootMargin: "-15% 0px -75% 0px" },
    );
    sections.forEach((sec) => observer.observe(sec));
    return () => observer.disconnect();
  }, [view, lang]);

  // Lightbox: Esc closes, arrows navigate, lock background scroll while open.
  useEffect(() => {
    if (lightbox === null) return;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setLightbox(null);
      else if (e.key === "ArrowLeft")
        setLightbox((p) => (p === null ? null : (p - 1 + gallery.length) % gallery.length));
      else if (e.key === "ArrowRight")
        setLightbox((p) => (p === null ? null : (p + 1) % gallery.length));
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = "";
      window.removeEventListener("keydown", onKey);
    };
  }, [lightbox]);

  function go(nextView: View) {
    setView(nextView);
    if (typeof window !== "undefined") {
      const base = process.env.NEXT_PUBLIC_BASE_PATH ?? "";
      const targetPath = nextView === "home" ? `${base}/` : `${base}/${nextView}/`;
      if (window.location.pathname !== targetPath) {
        window.history.pushState(null, "", targetPath);
      }
    }
    window.scrollTo({ top: 0, behavior: "smooth" });
  }

  function copyInstall() {
    navigator.clipboard?.writeText("npm install -g umadev").catch(() => undefined);
    setCopied(true);
    if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
    copyTimerRef.current = setTimeout(() => setCopied(false), 1500);
  }

  return (
    <div className={styles.shell}>
      <div className={styles.scrollProgress} ref={scrollProgressRef} aria-hidden="true"><i /></div>
      <div className={styles.gridBg} aria-hidden="true" />
      <div className={styles.topGlow} aria-hidden="true" />
      <div className={styles.pointerGlow} aria-hidden="true" />
      <div className={styles.scanlines} aria-hidden="true" />
      <div className={styles.noise} aria-hidden="true" />

      <nav className={styles.spaceNav}>
        <button className={styles.spaceNavBrand} type="button" onClick={() => go("home")}>
          <Image
            alt="UmaDev"
            className={styles.spaceNavBrandLogo}
            height={36}
            priority
            src={asset("/assets/umadev-icon.png")}
            width={36}
          />
          <span className={styles.spaceNavBrandText}>UmaDev</span>
        </button>

        <div className={styles.spaceNavLinks}>
          <button className={view === "home" ? styles.spaceNavLinkActive : styles.spaceNavLink} type="button" onClick={() => go("home")}>
            {t.nav.product}
          </button>
          <button className={view === "docs" ? styles.spaceNavLinkActive : styles.spaceNavLink} type="button" onClick={() => go("docs")}>
            {t.nav.docs}
          </button>
          <button className={view === "gallery" ? styles.spaceNavLinkActive : styles.spaceNavLink} type="button" onClick={() => go("gallery")}>
            {t.nav.gallery}
          </button>
          <button className={view === "contributors" ? styles.spaceNavLinkActive : styles.spaceNavLink} type="button" onClick={() => go("contributors")}>
            {t.nav.contributors}
          </button>
          <button className={view === "changelog" ? styles.spaceNavLinkActive : styles.spaceNavLink} type="button" onClick={() => go("changelog")}>
            {t.nav.changelog}
          </button>
        </div>

        <div className={styles.navActions}>
          <div className={styles.langSwitch} aria-label="Language switcher">
            <button
              className={lang === "zh" ? styles.langActive : styles.langButton}
              type="button"
              onClick={() => setLang("zh")}
            >
              中
            </button>
            <button
              className={lang === "en" ? styles.langActive : styles.langButton}
              type="button"
              onClick={() => setLang("en")}
            >
              EN
            </button>
          </div>
          <a className={`${styles.spaceNavCta} ${styles.spaceNavGithub} ${styles.umaMagnet}`} href={githubUrl} target="_blank" rel="noreferrer">
            <svg aria-hidden="true" viewBox="0 0 24 24">
              <path d="M12 2.4a9.8 9.8 0 0 0-3.1 19.1c.5.1.7-.2.7-.5v-1.9c-2.8.6-3.4-1.2-3.4-1.2-.5-1.2-1.1-1.5-1.1-1.5-.9-.6.1-.6.1-.6 1 0 1.6 1.1 1.6 1.1.9 1.6 2.4 1.1 2.9.8.1-.7.4-1.1.7-1.3-2.2-.3-4.6-1.1-4.6-4.9 0-1.1.4-2 1-2.7-.1-.3-.4-1.3.1-2.7 0 0 .8-.3 2.7 1a9.4 9.4 0 0 1 4.9 0c1.9-1.3 2.7-1 2.7-1 .5 1.4.2 2.4.1 2.7.6.7 1 1.6 1 2.7 0 3.8-2.3 4.6-4.6 4.9.4.3.7.9.7 1.8V21c0 .3.2.6.7.5A9.8 9.8 0 0 0 12 2.4Z" />
            </svg>
            <span>GitHub</span>
            <i aria-hidden="true">↗</i>
          </a>
          <button
            className={`${styles.spaceNavCta} ${styles.spaceNavWechat} ${styles.umaMagnet}`}
            type="button"
            onClick={() => setWechatOpen(true)}
          >
            <svg aria-hidden="true" viewBox="0 0 24 24">
              <path d="M9.3 4.2c-4 0-7.2 2.6-7.2 5.8 0 1.8 1 3.4 2.7 4.5l-.7 2.4 2.7-1.3c.8.2 1.6.3 2.5.3h.5a5.4 5.4 0 0 1-.2-1.5c0-3.5 3.3-6.3 7.4-6.3h.3c-1-2.3-4.1-3.9-8-3.9Zm-2.4 3a.9.9 0 1 1 0 1.8.9.9 0 0 1 0-1.8Zm4.8 0a.9.9 0 1 1 0 1.8.9.9 0 0 1 0-1.8Z" />
              <path d="M17 9.5c-3.4 0-6.2 2.2-6.2 4.9s2.8 4.9 6.2 4.9c.7 0 1.4-.1 2.1-.3l2.3 1.1-.6-2a4.6 4.6 0 0 0 2.2-3.7c0-2.7-2.7-4.9-6-4.9Zm-2.1 2.6a.8.8 0 1 1 0 1.6.8.8 0 0 1 0-1.6Zm4.1 0a.8.8 0 1 1 0 1.6.8.8 0 0 1 0-1.6Z" />
            </svg>
            <span>{lang === "zh" ? "微信群" : "WeChat"}</span>
          </button>
        </div>
      </nav>
      {wechatOpen && (
        <div
          className={styles.wechatOverlay}
          onClick={() => setWechatOpen(false)}
          role="dialog"
          aria-modal="true"
        >
          <div className={styles.wechatCard} onClick={(e) => e.stopPropagation()}>
            <button
              className={styles.wechatClose}
              type="button"
              onClick={() => setWechatOpen(false)}
              aria-label={lang === "zh" ? "关闭" : "Close"}
            >
              ×
            </button>
            <h3 className={styles.wechatTitle}>{lang === "zh" ? "官方微信群" : "Official WeChat Group"}</h3>
            <Image
              src="/assets/wechat-qr.png"
              alt={lang === "zh" ? "官方微信群二维码" : "Official WeChat group QR code"}
              width={240}
              height={240}
              className={styles.wechatQr}
            />
            <p className={styles.wechatHint}>
              {lang === "zh" ? "微信扫码加入官方交流群" : "Scan with WeChat to join the official group"}
            </p>
          </div>
        </div>
      )}

      <main className={styles.main}>
        {view === "home" && (
          <>
            {/* HERO SECTION */}
            <section className={styles.spaceHero}>
              <div className={styles.heroAtmosphere} aria-hidden="true" />
              <div className={styles.heroStage}>
                <div className={styles.heroCopy}>
                  <div className={styles.heroMeta}>
                    <span className={styles.heroMetaDot} />
                    <span className={styles.heroMetaText}>
                      {lang === "zh" ? "AI 开发团队协调器 · 本地运行" : "AI DEV TEAM ORCHESTRATOR · RUNS LOCAL"}
                    </span>
                  </div>

                  <h1 className={styles.heroWordmark}>
                    Uma<span>Dev</span>
                  </h1>

                  <h2 className={styles.heroStatement}>
                    {lang === "zh" ? (
                      <>一句需求，<br /><span>交付一个真项目。</span></>
                    ) : (
                      <>One requirement.<br /><span>A real product shipped.</span></>
                    )}
                  </h2>

                  <p className={styles.heroDescription}>
                    {lang === "zh"
                      ? "像真实开发团队一样，依次完成调研、PRD、架构、UI/UX、前后端、质量门和交付证明；实际编码由你已经登录的本机底座完成。"
                      : "Work like a real development team across research, PRD, architecture, UI/UX, frontend, backend, quality gates, and delivery proof — using the local CLI you are already signed into."}
                  </p>

                  <div className={styles.heroActionRow}>
                    <button className={styles.heroInstall} type="button" onClick={copyInstall}>
                      <span>$</span>
                      <code>npm install -g umadev</code>
                      <strong>{copied ? (lang === "zh" ? "已复制" : "COPIED") : (lang === "zh" ? "复制" : "COPY")}</strong>
                    </button>
                  </div>

                  <dl className={styles.heroFacts}>
                    <div><dt>3</dt><dd>{lang === "zh" ? "本机底座" : "LOCAL BASES"}</dd></div>
                    <div><dt>9</dt><dd>{lang === "zh" ? "交付阶段" : "DELIVERY PHASES"}</dd></div>
                    <div><dt>113</dt><dd>{lang === "zh" ? "治理检查" : "GOVERNANCE CHECKS"}</dd></div>
                  </dl>
                </div>

                <div className={styles.heroSystem} aria-label={lang === "zh" ? "UmaDev 交付流水线动态预览" : "UmaDev delivery pipeline preview"}>
                  <div className={styles.heroSystemBar}>
                    <div className={styles.heroSystemLights}><i /><i /><i /></div>
                    <span>UMADEV / LIVE RUN</span>
                    <strong><i /> ONLINE</strong>
                  </div>

                  <div className={styles.heroPrompt}>
                    <span>{lang === "zh" ? "你的需求" : "YOUR BRIEF"}</span>
                    <p>{lang === "zh" ? "做一个能上线的产品，而不只是一段代码。" : "Build a product that can ship, not just a piece of code."}</p>
                  </div>

                  <div className={styles.heroPhaseTrack} aria-hidden="true">
                    {t.stages.map((stage, index) => (
                      <i
                        key={stage.key}
                        className={index === activeStageIdx ? styles.heroPhaseActive : index < activeStageIdx ? styles.heroPhaseDone : undefined}
                      />
                    ))}
                  </div>

                  {(() => {
                    const stage = t.stages[activeStageIdx];
                    return (
                      <div className={styles.heroLiveStage} key={stage.key}>
                        <div className={styles.heroStageIndex}>{String(activeStageIdx + 1).padStart(2, "0")}</div>
                        <div className={styles.heroStageCopy}>
                          <span>{lang === "zh" ? "当前角色正在工作" : "ACTIVE TEAM ROLE"}</span>
                          <h3>{stage.label}</h3>
                          <p>{stage.role}</p>
                        </div>
                        <span className={styles.heroStagePulse} />
                      </div>
                    );
                  })()}

                  <div className={styles.heroOutputs}>
                    <div className={styles.heroOutputsHead}>
                      <span>{lang === "zh" ? "实时产物" : "LIVE ARTIFACTS"}</span>
                      <small>{lang === "zh" ? "写入项目" : "WRITING TO PROJECT"}</small>
                    </div>
                    {t.stages[activeStageIdx].files.slice(0, 3).map((file, index) => (
                      <div className={styles.heroOutputFile} key={file}>
                        <i>{index + 1}</i>
                        <code>{file}</code>
                        <span>{index === 0 ? "SYNC" : "READY"}</span>
                      </div>
                    ))}
                  </div>

                  <div className={styles.heroSystemFoot}>
                    <div><span>BASE</span><strong>CLAUDE CODE / CODEX / OPENCODE</strong></div>
                    <div><span>GATE</span><strong>QUALITY 90+</strong></div>
                    <div><span>PROOF</span><strong>JSONL + SCORECARD</strong></div>
                  </div>
                </div>
              </div>
              <a className={styles.heroScrollCue} href="#pipeline" aria-label={lang === "zh" ? "滚动查看交付流水线" : "Scroll to the delivery pipeline"}>
                <span>{lang === "zh" ? "查看完整交付系统" : "EXPLORE THE DELIVERY SYSTEM"}</span>
                <i />
              </a>
            </section>

            {/* MARQUEE */}
            <section className={styles.marqueeSection}>
              <div className={styles.marqueeContainer}>
                <div className={styles.marqueeTrack}>
                  {Array.from({ length: 4 }).flatMap((_, outerIdx) => (
                    <React.Fragment key={outerIdx}>
                      <span className={styles.marqueeText}>{lang === "zh" ? "驱动你已登录的底座 —" : "DRIVES YOUR LOGGED-IN BASE —"}</span>
                      <span className={styles.marqueeItem}>Claude Code</span><span className={styles.marqueeSep}>◆</span>
                      <span className={styles.marqueeItem}>Codex</span><span className={styles.marqueeSepPurple}>◆</span>
                      <span className={styles.marqueeItem}>OpenCode</span><span className={styles.marqueeSep}>◆</span>
                      <span className={styles.marqueeText}>{lang === "zh" ? "不接外部 API · 不保存登录 · 底座用它自己的模型" : "NO EXTERNAL API KEY · NO LOGIN SAVED · BASE USES ITS OWN MODEL"}</span><span className={styles.marqueeSepPurple}>◆</span>
                    </React.Fragment>
                  ))}
                </div>
              </div>
            </section>

            {/* COOPERATIVE COMMUNITIES */}
            <section className={styles.partnersSection} id="community">
              <div className={styles.partnersContainer}>
                <div className={styles.communityHalo} aria-hidden="true" />
                <h2 className={styles.partnersTitle} data-title={t.partners.title}>{t.partners.title}</h2>
                {["top", "bottom"].map((position) => {
                  const items = position === "top" ? t.partners.list : [...t.partners.list].reverse();
                  return (
                    <div key={position} className={`${styles.communityBelt} ${position === "top" ? styles.communityBeltTop : styles.communityBeltBottom}`}>
                      <div className={styles.communityBeltTrack}>
                        {[...items, ...items].map((partner, idx) => (
                          <a
                            key={`${position}-${partner.logoName}-${idx}`}
                            aria-label={partner.name}
                            href={partner.url}
                            target={partner.url === "#" ? undefined : "_blank"}
                            rel={partner.url === "#" ? undefined : "noopener noreferrer"}
                            className={styles.communityLogo}
                            data-partner={partner.logoName}
                            onClick={(event) => {
                              if (partner.url === "#") event.preventDefault();
                            }}
                          >
                            <Image
                              src={asset(`/assets/partners/${partner.logoName === "paopai" ? "paopai-transparent" : partner.logoName}.png?v=5`)}
                              alt={partner.name}
                              width={220}
                              height={92}
                              loading="eager"
                            />
                          </a>
                        ))}
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>

            {/* ROLE / PROBLEM */}
            <section className={styles.painSection}>
              <h2 className={styles.painTitle}>
                {lang === "zh" ? "解决问题" : "What it solves"}
              </h2>

              <div className={styles.painGrid}>
                <div className={`${styles.painCard} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <span className={styles.painCardTagCross}>✕ {lang === "zh" ? "常见" : "COMMON"}</span>
                  <p className={styles.painCardText}>
                    {lang === "zh" ? "AI 一上来就写代码，没有 PRD、没有架构、没有验收标准。" : "LLMs start coding immediately without any PRD, architecture specifications, or acceptance criteria."}
                  </p>
                </div>
                <div className={`${styles.painCard} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <span className={styles.painCardTagCross}>✕ {lang === "zh" ? "常见" : "COMMON"}</span>
                  <p className={styles.painCardText}>
                    {lang === "zh" ? "前端做完了，后端接口路径对不上。UI 像模板，颜色字体很随意。" : "Frontend gets built, but backend API paths mismatch. UI looks generic with template-like aesthetics."}
                  </p>
                </div>
                <div className={`${styles.painCard} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <span className={styles.painCardTagCross}>✕ {lang === "zh" ? "常见" : "COMMON"}</span>
                  <p className={styles.painCardText}>
                    {lang === "zh" ? "写了占位代码、假数据、TODO，却说「完成了」。改一次需求上下文就乱。" : "AI writes placeholder code, fake mocks, and TODOs, yet claims done. A single change causes context drift."}
                  </p>
                </div>
                <div className={`${styles.painCardActive} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <span className={styles.painCardTagCheck}>✓ UmaDev {lang === "zh" ? "角色阵容" : "ROLES"}</span>
                  <p className={styles.painCardActiveText}>
                    {lang === "zh" ? "产品经理 + 架构师 + UI/UX 审稿人 + 技术负责人 + QA + 交付经理。" : "Product Manager + Architect + UI/UX Designer + Tech Lead + QA + DevOps."}
                  </p>
                </div>
                <div className={`${styles.painCardActive} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <span className={styles.painCardTagCheck}>✓ UmaDev {lang === "zh" ? "交付闭环" : "CLOSED-LOOP"}</span>
                  <p className={styles.painCardActiveText}>
                    {lang === "zh" ? "你只输入一句需求，它负责把「AI 写代码」变成一个完整、可上线、可审计的项目交付过程。" : "You enter a single prompt, and it orchestrates the process into a complete, shippable, auditable delivery."}
                  </p>
                </div>
                <div className={`${styles.painCardActive} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <span className={styles.painCardTagCheck}>✓ UmaDev {lang === "zh" ? "质量红线" : "QUALITY GATES"}</span>
                  <p className={styles.painCardActiveText}>
                    {lang === "zh" ? "113 条覆盖安全、接口对账与 UI 规范的治理规则，编译测试全绿、审计证据链齐全才放行交付。" : "113 rules check safety, contracts, and UI code. Only releases when tests pass and audit log is verified."}
                  </p>
                </div>
              </div>
            </section>

            {/* VERTICAL PIPELINE SECTION */}
            <section id="pipeline" className={styles.pipelineSection}>
              <div className={styles.pipelineHeader}>
                <h2 className={styles.pipelineTitle}>
                  {lang === "zh" ? "交付流水线" : "Delivery pipeline"}
                </h2>
              </div>

              <div
                className={styles.pipelineContainer}
                onMouseEnter={() => setAutoplay(false)}
                onMouseLeave={() => setAutoplay(true)}
              >
                {/* Horizontal Progress Bar */}
                <div className={styles.pipelineProgressBarWrapper}>
                  <div
                    className={styles.pipelineProgressBarLine}
                    style={{
                      "--progress-width": `${(activeStageIdx / Math.max(t.stages.length - 1, 1)) * 100}%`
                    } as React.CSSProperties}
                  />
                  <div className={styles.pipelineProgressNodes}>
                    {t.stages.map((stage, index) => {
                      const isGate = stage.gate;
                      const color = isGate ? "#ff2a85" : "#00d2ff";
                      const stepNum = String(index + 1).padStart(2, "0");
                      const isActive = index === activeStageIdx;
                      const isCompleted = index <= activeStageIdx;

                      return (
                        <button
                          key={stage.key}
                          type="button"
                          className={`${styles.pipelineNodeButton} ${isActive ? styles.pipelineNodeActive : ""} ${isCompleted ? styles.pipelineNodeCompleted : ""}`}
                          onClick={() => setActiveStageIdx(index)}
                          style={{
                            "--node-color": color,
                            "--node-color-rgba": `${color}2a`
                          } as React.CSSProperties}
                        >
                          <span className={styles.pipelineNodeDot}>{stepNum}</span>
                          <span className={styles.pipelineNodeLabel}>{stage.label}</span>
                        </button>
                      );
                    })}
                  </div>
                </div>

                {/* Dashboard Detail Panel */}
                <div className={styles.pipelineDetailPanel}>
                  {(() => {
                    const stage = t.stages[activeStageIdx];
                    const isGate = stage.gate;
                    const isMicro = stage.key === "clarify";
                    const color = isGate ? "#ff2a85" : "#00d2ff";

                    return (
                      <div className={styles.pipelinePanelContent} key={stage.key}>
                        <div className={styles.pipelinePanelLeft}>
                          <div className={styles.pipelineMetaRow}>
                            <span className={styles.pipelineName}>{stage.label}</span>
                            <span
                              className={`${styles.pipelineBadge} ${styles.mono}`}
                              style={{ background: `${color}14`, color: color }}
                            >
                              {stage.key}
                            </span>
                            {isGate && <span className={styles.pipelineGateLabel}>{lang === "zh" ? "确认门禁" : "CONFIRM GATE"}</span>}
                            {isMicro && <span className={styles.pipelineMicroLabel}>{lang === "zh" ? "微阶段" : "MICRO PHASE"}</span>}
                          </div>
                          <div className={styles.pipelineDetailText} style={{ marginTop: "14px", fontSize: "16px", minHeight: "60px" }}>
                            {stage.role}
                          </div>
                        </div>

                        <div className={styles.pipelinePanelRight}>
                          <div className={styles.pipelineExplorer}>
                            <div className={styles.pipelineExplorerHeader}>
                              <span style={{ fontSize: "12px", textTransform: "uppercase", letterSpacing: "0.08em", color: "#8a8f9c" }}>
                                {lang === "zh" ? "OUTPUT / 生成交付产物" : "OUTPUT / DELIVERABLES"}
                              </span>
                            </div>
                            <div className={styles.pipelineExplorerList}>
                              {stage.files.map((file) => (
                                <div key={file} className={styles.pipelineExplorerItem}>
                                  <span style={{ color: color, fontSize: "10px", fontFamily: "var(--font-mono), monospace" }}>FILE</span>
                                  <span className={`${styles.pipelineExplorerName} ${styles.mono}`}>{file}</span>
                                </div>
                              ))}
                            </div>
                          </div>
                        </div>
                      </div>
                    );
                  })()}
                </div>
              </div>
            </section>

            {/* GOVERNANCE STATS */}
            <section id="governance" className={styles.statsSection}>
              <div className={styles.statsSectionHeader}>
                <div style={{ maxWidth: "640px" }}>
                  <h2 className={styles.statsSectionTitle}>
                    {lang === "zh" ? "治理与质量" : "Governance & quality"}
                  </h2>
                </div>
              </div>

              <div className={styles.statsGrid}>
                {[
                  { count: 113, label: lang === "zh" ? "条治理规则 + 审计" : "Governance Rules + Audit", color: "#00d2ff", active: false },
                  { count: 416, label: lang === "zh" ? "份内置知识文件" : "Embedded Knowledge Files", color: "#ff2a85", active: false },
                  { count: 9, label: lang === "zh" ? "个可治理交付阶段" : "Verifiable Delivery Phases", color: "#00d2ff", active: false },
                  { count: 90, label: lang === "zh" ? "默认质量门通过线" : "Default Quality Gate Bar", color: "#00d2ff", active: true, suffix: lang === "zh" ? "分" : " pts" },
                ].map((item, idx) => (
                  <div
                    key={idx}
                    className={`${item.active ? styles.statCardActive : styles.statCard} ${styles.reveal} ${styles.tilt}`}
                    onMouseMove={handleTiltMove}
                    onMouseLeave={handleTiltLeave}
                    style={{ transitionDelay: `${idx * 40}ms` }}
                  >
                    <div style={{ display: "flex", alignItems: "baseline", gap: "6px" }}>
                      <div
                        className={`${styles.statNumber} ${styles.mono}`}
                        style={{ color: item.color }}
                        data-count={item.count}
                      >
                        {item.count}
                      </div>
                      {item.suffix && <span style={{ fontSize: "24px", color: item.color }}>{item.suffix}</span>}
                    </div>
                    <div style={{ marginTop: "12px", fontSize: "15px", color: item.active ? "#e2ff8a" : "#b4b9c4" }}>{item.label}</div>
                  </div>
                ))}
              </div>
            </section>

            {/* CRATES SECTION */}
            <section className={styles.cratesSection}>
              <div className={styles.cratesHeader}>
                <h2 className={styles.cratesTitle}>
                  {lang === "zh" ? "Rust 工作区" : "Rust workspace"}
                </h2>
              </div>

              <div className={styles.cratesGrid}>
                {[
                  ["umadev", lang === "zh" ? "主程序" : "CLI Core", lang === "zh" ? "CLI · TUI · doctor · hook · CI · MCP" : "CLI · TUI · doctor · hook · CI · MCP"],
                  ["umadev-spec", lang === "zh" ? "规则说明书" : "Spec Definition", lang === "zh" ? "UMADEV_HOST_SPEC_V1 数据" : "UMADEV_HOST_SPEC_V1 static spec representation"],
                  ["umadev-governance", lang === "zh" ? "质检和红线" : "Governance Compliance", lang === "zh" ? "113 条规则 · 审计 · 合规映射" : "113 rules check · audit trails · SOC 2 compliance mapper"],
                  ["umadev-agent", lang === "zh" ? "项目总监" : "Orchestrator Agent", lang === "zh" ? "9 阶段 runner · gate · 质量门" : "9-phase workflow runner · gate triggers · quality checkpoint"],
                  ["umadev-runtime", lang === "zh" ? "统一大脑接口" : "Runtime Interface", lang === "zh" ? "Offline · HTTP runtime · trait" : "Offline templates · HTTP channel drivers · base trait"],
                  ["umadev-host", lang === "zh" ? "驱动外部 CLI" : "Base CLI Drivers", lang === "zh" ? "Claude Code · Codex · OpenCode" : "Claude Code · Codex · OpenCode subprocess hooks"],
                  ["umadev-contract", lang === "zh" ? "API 对账员" : "Contract Alignment", lang === "zh" ? "OpenAPI 契约 · 路径校验" : "OpenAPI verification · frontend Axios / fetch path check"],
                  ["umadev-knowledge", lang === "zh" ? "知识检索" : "RAG Hybrid Search", lang === "zh" ? "BM25 · chunk · 可选 vector" : "BM25 lexical channel · Candle vector embeddings RRF fuser"],
                  ["umadev-tui", lang === "zh" ? "终端界面" : "Terminal TUI Layout", lang === "zh" ? "ratatui 聊天 UI · 预览部署" : "Ratatui interactive chat · local preview logs · action diffs"],
                  ["umadev-i18n", lang === "zh" ? "多语言" : "Internationalization", lang === "zh" ? "简中 · 繁中 · English" : "zh-CN · zh-TW · English translation dicts"],
                ].map(([name, role, desc], i) => (
                  <div
                    key={name}
                    className={`${styles.crateCard} ${styles.reveal} ${styles.tilt}`}
                    onMouseMove={handleTiltMove}
                    onMouseLeave={handleTiltLeave}
                    style={{ transitionDelay: `${i * 40}ms` }}
                  >
                    <div className={styles.crateName}>{name}</div>
                    <div className={styles.crateRole}>{role}</div>
                    <div className={styles.crateDesc}>{desc}</div>
                  </div>
                ))}
              </div>
            </section>

            {/* KNOWLEDGE RETRIEVAL SECTION */}
            <section id="knowledge" className={styles.retrieveSection}>
              <div className={styles.retrieveGrid}>
                <div className={styles.reveal}>
                  <h2 className={styles.retrieveTitle}>
                    {lang === "zh" ? "工程知识库" : "Knowledge base"}
                  </h2>
                  <div className={`${styles.mono} ${styles.reveal}`} style={{ display: "flex", flexDirection: "column", gap: "8px", fontSize: "13.5px", color: "#8a8f9c", marginTop: "24px" }}>
                    <span><span style={{ color: "#00d2ff" }}>$</span> umadev knowledge-manage add ./team-docs</span>
                    <span><span style={{ color: "#00d2ff" }}>$</span> umadev knowledge-manage search {lang === "zh" ? '"支付 webhook 幂等"' : '"payment webhook idempotency"'}</span>
                  </div>
                </div>

                <div className={`${styles.reveal}`} style={{ position: "relative", padding: "30px", borderRadius: "20px", background: "#0c0e14", border: "1px solid rgba(255, 255, 255, 0.08)", overflow: "hidden" }}>
                  <div className={styles.mono} style={{ display: "flex", flexDirection: "column", gap: "11px", fontSize: "13px" }}>
                    {[
                      { num: "01", name: lang === "zh" ? "分词" : "Tokenization", desc: lang === "zh" ? "中文 + 英文" : "CJK bigram + English", color: "#ff2a85" },
                      { num: "02", name: lang === "zh" ? "BM25 检索" : "BM25 Retrieval", desc: lang === "zh" ? "top-k 命中工程标准" : "top-k lexical matches", color: "#00d2ff" },
                      { num: "03", name: lang === "zh" ? "向量检索" : "Vector Search", desc: lang === "zh" ? "OPENAI_EMBED_KEY 可选" : "local candles / optional OpenAI API", color: "#ff2a85" },
                      { num: "04", name: lang === "zh" ? "RRF 融合排序" : "RRF Fusion Re-ranking", desc: lang === "zh" ? "合并两路结果" : "reciprocal rank fusion blending", color: "#00d2ff" },
                      { num: "05", name: lang === "zh" ? "注入当前阶段 prompt" : "Prompt Context Injection", desc: lang === "zh" ? "→ 底座" : "→ base CLI input stream", color: "#00d2ff" },
                    ].map((step) => (
                      <div key={step.num} style={{ display: "flex", alignItems: "center", gap: "12px", padding: "12px 14px", borderRadius: "10px", background: "#0a0b10", borderLeft: `2px solid ${step.color}` }}>
                        <span style={{ color: step.color, fontSize: "12px" }}>{step.num}</span>
                        <span style={{ color: "#f4f6f2", fontSize: "13.5px", flex: 1 }}>{step.name}</span>
                        <span style={{ color: "#8a8f9c", fontSize: "12px" }}>{step.desc}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </section>

            {/* PROOF OF DELIVERY */}
            <section id="deliver" className={styles.deliverSection}>
              <div className={styles.deliverHeader}>
                <h2 className={styles.deliverTitle}>
                  {lang === "zh" ? "交付证据" : "Delivery proof"}
                </h2>
              </div>

              <div className={styles.deliverGrid}>
                <div className={`${styles.deliverCard} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <div className={styles.deliverCardPath}>output/</div>
                  <h3 className={styles.deliverCardTitle}>{lang === "zh" ? "过程文档" : "Process Documents"}</h3>
                  <p className={styles.deliverCardDesc}>clarify · research · prd · architecture · uiux · execution-plan · quality-gate</p>
                </div>
                <div className={`${styles.deliverCard} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <div className={styles.deliverCardPath}>.umadev/audit/</div>
                  <h3 className={styles.deliverCardTitle}>{lang === "zh" ? "证据链" : "Evidence Logs"}</h3>
                  <p className={styles.deliverCardDesc}>tool-calls.jsonl · frontend-api-calls.jsonl · verify.jsonl · 状态与合规映射</p>
                </div>
                <div className={`${styles.deliverCardActive} ${styles.reveal} ${styles.tilt}`} onMouseMove={handleTiltMove} onMouseLeave={handleTiltLeave}>
                  <div className={styles.deliverCardPathActive}>release/</div>
                  <h3 className={styles.deliverCardTitleActive}>{lang === "zh" ? "最终交付包" : "Release Package"}</h3>
                  <p className={styles.deliverCardDescActive}>
                    {lang === "zh" ? "proof-pack-*.zip 与 scorecard-*.html —— 给团队、客户、审计方看的交付证明。" : "proof-pack-*.zip and scorecard-*.html —— evidence verification package for clients and auditors."}
                  </p>
                </div>
              </div>
            </section>

            {/* SPACE CTA */}
            <section className={styles.spaceCta}>
              <div className={styles.spaceCtaOverlay} />
              <div className={styles.spaceCtaContent}>
                <h2 className={styles.spaceCtaTitle}>
                  {lang === "zh" ? "开始交付" : "Start shipping"}
                </h2>
                <div className={styles.spaceCtaConsole}>
                  <span className={styles.spaceCtaConsolePrompt}>$</span> npm install -g umadev
                  <button className={`${styles.umaCopy} ${styles.umaMagnet}`} style={{ marginLeft: "12px", padding: "6px 12px", borderRadius: "8px", border: "none", background: "#00d2ff", color: "#07080c", fontFamily: "var(--font-mono), monospace", fontSize: "12px", fontWeight: 600, cursor: "pointer" }} onClick={copyInstall} onMouseMove={handleMagnetMove} onMouseLeave={handleMagnetLeave}>
                    {copied ? (lang === "zh" ? "已复制 ✓" : "Copied ✓") : (lang === "zh" ? "复制" : "Copy")}
                  </button>
                </div>
                <div className={styles.spaceCtaActions}>
                  <a className={`${styles.spaceCtaBtn1} ${styles.umaMagnet}`} href="https://github.com/umacloud/umadev" target="_blank" rel="noreferrer" onMouseMove={handleMagnetMove} onMouseLeave={handleMagnetLeave}>
                    {lang === "zh" ? "在 GitHub 查看" : "View on GitHub"}
                  </a>
                  <button className={`${styles.spaceCtaBtn2} ${styles.umaMagnet}`} type="button" onClick={() => go("docs")} onMouseMove={handleMagnetMove} onMouseLeave={handleMagnetLeave}>
                    {lang === "zh" ? "阅读文档" : "Read Docs"}
                  </button>
                </div>
              </div>
              <div className={styles.spaceCtaArt} aria-hidden="true">
                <div className={styles.spaceCtaOrbit} />
                <Image
                  alt=""
                  className={styles.spaceCtaIp}
                  height={530}
                  src={asset("/assets/umadev/generated/changelog-ip.png")}
                  unoptimized
                  width={428}
                />
              </div>
            </section>
          </>
        )}

        {view === "docs" && (
          <section className={styles.docsPage}>
            <PageHero title={t.docsPage.title} sub={t.docsPage.sub} variant="docs" />
            <div className={styles.docsLayout}>
              <aside className={styles.docsNav}>
                {docCats.map((cat) => (
                  <div key={cat.cat}>
                    <h3>{cat.cat}</h3>
                    {cat.items.map((item) => (
                      <button
                        className={item.id === activeDoc.id ? styles.docActive : styles.docLink}
                        key={item.id}
                        type="button"
                        onClick={() => {
                          setDocId(item.id);
                          document
                            .getElementById(item.id)
                            ?.scrollIntoView({ behavior: "smooth", block: "start" });
                        }}
                      >
                        {item.title}
                      </button>
                    ))}
                  </div>
                ))}
              </aside>
              <article className={styles.docArticle}>
                {docCats
                  .flatMap((cat) => cat.items)
                  .map((item) => (
                    <section
                      className={styles.docSection}
                      data-doc-section
                      id={item.id}
                      key={item.id}
                    >
                      <h2>{item.title}</h2>
                      {item.blocks.map((block, index) => (
                        <DocBlockView block={block} key={index} />
                      ))}
                    </section>
                  ))}
              </article>
            </div>
          </section>
        )}

        {view === "changelog" && (
          <section className={styles.logPage}>
            <PageHero title={t.logPage.title} sub={t.logPage.sub} variant="changelog" />
            <ol className={styles.releaseTimeline}>
              {releases[lang].map((release, index) => (
                <ReleaseEntry
                  key={release.ver}
                  release={release}
                  isCurrent={index === 0}
                  latestLabel={t.logPage.current}
                  moreLabel={t.logPage.more}
                  lessLabel={t.logPage.less}
                />
              ))}
            </ol>
          </section>
        )}

        {view === "gallery" && (
          <section className={`${styles.docsPage} ${styles.galleryPage}`}>
            <PageHero title={t.galleryPage.title} sub={t.galleryPage.sub} variant="gallery" />
            <div className={styles.galleryGrid}>
              {gallery.map((src, i) => (
                <button
                  className={`${styles.galleryItem} ${styles.tilt}`}
                  key={src}
                  onClick={() => setLightbox(i)}
                  type="button"
                  onMouseMove={handleTiltMove}
                  onMouseLeave={handleTiltLeave}
                >
                  <Image alt={`UmaDev IP ${i + 1}`} className={styles.galleryImg} height={420} priority={i === 0} src={src} unoptimized width={420} />
                </button>
              ))}
            </div>
          </section>
        )}

        {view === "contributors" && (
          <section className={styles.contributorsPage}>
            <PageHero title={t.contributorsPage.title} sub={t.contributorsPage.sub} variant="contributors" />

            <div className={styles.contributorsMatrix}>
              {[
                { ...t.contributors.featured, isVip: true, vipText: lang === "zh" ? "创始荣誉席" : "Founding Honor", rank: "01" },
                ...t.contributors.list.filter((item) => (item as { isVip?: boolean }).isVip).map((item) => ({
                  ...item,
                  isVip: true,
                  vipText: lang === "zh" ? "核心荣誉席" : "Core Honor",
                  rank: "02"
                })),
                ...t.contributors.list.filter((item) => !(item as { isVip?: boolean }).isVip).map((item) => ({
                  ...item,
                  isVip: false,
                  vipText: undefined,
                  rank: undefined
                }))
              ].map((member) => (
                <div
                  key={member.avatarKey}
                  className={`${styles.matrixCard} ${styles.tilt}`}
                  data-rank={member.rank}
                  data-vip={member.isVip ? "true" : undefined}
                  onMouseMove={handleTiltMove}
                  onMouseLeave={handleTiltLeave}
                >
                  <div className={styles.matrixCardGlow} />
                  {member.isVip && <div className={styles.matrixVipOrbit} />}
                  {member.isVip && <span className={styles.matrixVipIndex}>{member.rank}</span>}
                  {member.isVip && <div className={styles.matrixVipBadge}>{member.vipText}</div>}
                  <div className={styles.matrixAvatarWrapper}>
                    <Image
                      src={asset(`/assets/contributors/${member.avatarKey}.png?v=2`)}
                      alt={member.name}
                      width={88}
                      height={88}
                      priority={member.isVip}
                      className={styles.matrixAvatarImg}
                    />
                  </div>
                  <div className={styles.matrixTextInfo}>
                    <span className={styles.matrixName}>{member.name}</span>
                    <span className={styles.matrixRole}>{member.role}</span>
                  </div>
                </div>
              ))}
            </div>
          </section>
        )}
      </main>

      {lightbox !== null && (
        <div aria-modal="true" className={styles.lightbox} onClick={() => setLightbox(null)} role="dialog">
          <button aria-label="Close" className={styles.lightboxClose} onClick={() => setLightbox(null)} type="button">
            <svg aria-hidden="true" viewBox="0 0 24 24"><path d="m6 6 12 12M18 6 6 18" /></svg>
          </button>
          <div className={styles.lightboxStage} onClick={(event) => event.stopPropagation()}>
            <Image
              alt={`UmaDev IP ${lightbox + 1}`}
              className={styles.lightboxImg}
              height={1100}
              src={gallery[lightbox]}
              unoptimized
              width={1100}
            />
          </div>
          <div className={styles.lightboxControls} onClick={(event) => event.stopPropagation()}>
            <button
              aria-label="Previous"
              className={styles.lightboxNav}
              data-side="prev"
              onClick={() => setLightbox((p) => (p === null ? null : (p - 1 + gallery.length) % gallery.length))}
              type="button"
            >
              <svg aria-hidden="true" viewBox="0 0 24 24"><path d="m15 5-7 7 7 7" /></svg>
              <span>{lang === "zh" ? "上一张" : "Previous"}</span>
            </button>
            <span className={styles.lightboxCount}>
              <b>{String(lightbox + 1).padStart(2, "0")}</b>
              <i>/</i>
              <span>{String(gallery.length).padStart(2, "0")}</span>
            </span>
            <button
              aria-label="Next"
              className={styles.lightboxNav}
              data-side="next"
              onClick={() => setLightbox((p) => (p === null ? null : (p + 1) % gallery.length))}
              type="button"
            >
              <span>{lang === "zh" ? "下一张" : "Next"}</span>
              <svg aria-hidden="true" viewBox="0 0 24 24"><path d="m9 5 7 7-7 7" /></svg>
            </button>
          </div>
        </div>
      )}

      <footer className={styles.spaceFooter}>
        <div className={styles.spaceFooterBrand}>
          <Image
            alt=""
            aria-hidden="true"
            className={styles.spaceFooterBrandLogo}
            height={30}
            src={asset("/assets/umadev-icon.png")}
            width={30}
          />
          <span className={styles.spaceFooterBrandText}>
            {lang === "zh" ? "UmaDev · umacloud 开源项目 · MIT License" : "UmaDev · an umacloud open-source project · MIT License"}
          </span>
        </div>
        <div className={`${styles.spaceFooterLink} ${styles.mono}`}>umadev.goder.ai</div>
      </footer>
    </div>
  );

}

/** Map a localized change tag (zh / en) to its colored chip class. */
function tagClass(tag: string) {
  const map: Record<string, string> = {
    新增: styles.tagAdded,
    Added: styles.tagAdded,
    改进: styles.tagImproved,
    Improved: styles.tagImproved,
    安全: styles.tagSecurity,
    Security: styles.tagSecurity,
    修复: styles.tagFixed,
    Fixed: styles.tagFixed,
    平台: styles.tagPlatform,
    Platform: styles.tagPlatform,
    变更: styles.tagChanged,
    Changed: styles.tagChanged,
  };
  return `${styles.releaseTag} ${map[tag] ?? styles.tagImproved}`;
}

/** One changelog release. Array order determines the latest highlighted entry. */
type Release = {
  ver: string;
  date: string;
  title: string;
  current?: boolean;
  changes: readonly (readonly [string, string])[];
};

/** How many changes a release shows before the "show more" toggle appears,
 *  and how many stay visible while collapsed — keeps long entries scannable. */
const RELEASE_PEEK = 5;
const RELEASE_COLLAPSE_AT = 6;

/** A single changelog entry: a timeline node (version + date + Latest pill) on
 *  the rail, a short title and well-spaced, tagged changes on the right. Long
 *  entries collapse behind a "show more" toggle so the page stays scannable. */
function ReleaseEntry({
  release,
  isCurrent,
  latestLabel,
  moreLabel,
  lessLabel,
}: {
  release: Release;
  isCurrent: boolean;
  latestLabel: string;
  moreLabel: string;
  lessLabel: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const collapsible = release.changes.length > RELEASE_COLLAPSE_AT;
  const shown = !collapsible || expanded ? release.changes : release.changes.slice(0, RELEASE_PEEK);
  const hiddenCount = release.changes.length - shown.length;

  return (
    <li className={`${styles.releaseEntry} ${isCurrent ? styles.releaseEntryCurrent : ""}`}>
      <div className={styles.releaseMeta}>
        <div className={styles.releaseVer}>{release.ver}</div>
        <time className={styles.releaseDate}>{release.date}</time>
        {isCurrent && <span className={styles.releaseLatest}>{latestLabel}</span>}
      </div>
      <div className={styles.releaseBody}>
        <h2 className={styles.releaseTitle}>{release.title}</h2>
        <ul className={styles.releaseChanges}>
          {shown.map(([tag, text]) => (
            <li key={`${tag}-${text}`} className={styles.releaseChange}>
              <span className={tagClass(tag)}>{tag}</span>
              <p>{text}</p>
            </li>
          ))}
        </ul>
        {collapsible && (
          <button
            type="button"
            className={styles.releaseToggle}
            onClick={() => setExpanded((v) => !v)}
            aria-expanded={expanded}
          >
            {expanded ? lessLabel : moreLabel.replace("{n}", String(hiddenCount))}
            <Chevron up={expanded} />
          </button>
        )}
      </div>
    </li>
  );
}

/** Small chevron used by the show-more toggle (rotates when expanded). */
function Chevron({ up }: { up: boolean }) {
  return (
    <svg
      aria-hidden="true"
      width="11"
      height="11"
      viewBox="0 0 16 16"
      fill="none"
      className={up ? styles.chevronUp : styles.chevronDown}
    >
      <path d="M4 6l4 4 4-4" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}



type PageHeroVariant = "docs" | "gallery" | "contributors" | "changelog";

const pageHeroVisuals: Record<PageHeroVariant, { art: string; ip: string; label: string; metrics: readonly string[] }> = {
  docs: {
    art: "/assets/umadev/vectors/docs-orbit.svg",
    ip: "/assets/umadev/generated/docs-ip.png",
    label: "UmaDev / Documentation",
    metrics: ["113 GOVERNANCE", "09 PHASES", "03 LOCAL BASES"],
  },
  gallery: {
    art: "/assets/umadev/vectors/gallery-aperture.svg",
    ip: "/assets/umadev/generated/gallery-ip.png",
    label: "UmaDev / Visual Archive",
    metrics: ["45 IP SCENES", "ORIGINAL SERIES", "VISUAL ARCHIVE"],
  },
  contributors: {
    art: "/assets/umadev/vectors/contributor-constellation.svg",
    ip: "/assets/umadev/generated/honor-ip.png",
    label: "UmaDev / Open Source",
    metrics: ["MIT LICENSE", "COMMUNITY BUILT", "SPECIAL THANKS"],
  },
  changelog: {
    art: "/assets/umadev/vectors/changelog-flow.svg",
    ip: "/assets/umadev/generated/changelog-ip.png",
    label: "UmaDev / Release Stream",
    metrics: [`CURRENT v${releases.en[0].ver}`, "RUST BINARY", "VERIFIED RELEASES"],
  },
};

function PageHero({ title, sub, variant }: { title: string; sub: string; variant?: PageHeroVariant }) {
  if (variant) {
    const visual = pageHeroVisuals[variant];
    return (
      <header className={`${styles.pageHero} ${styles.routeHero}`} data-hero={variant}>
        <div className={styles.routeHeroCopy}>
          <span>{visual.label}</span>
          <h1>{title}</h1>
          <p>{sub}</p>
        </div>
        <div aria-hidden="true" className={styles.routeHeroArt}>
          <Image
            alt=""
            className={styles.routeHeroVector}
            height={800}
            priority
            src={asset(visual.art)}
            unoptimized
            width={800}
          />
          <Image
            alt=""
            className={styles.routeHeroIp}
            height={1254}
            priority
            src={asset(visual.ip)}
            unoptimized
            width={1254}
          />
          <div className={styles.routeHeroMetrics}>
            {visual.metrics.map((metric) => <span key={metric}>{metric}</span>)}
          </div>
        </div>
      </header>
    );
  }

  return (
    <header className={styles.pageHero}>
      <span>UmaDev</span>
      <h1>{title}</h1>
      <p>{sub}</p>
    </header>
  );
}

function DocCode({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  const handleCopy = () => {
    navigator.clipboard.writeText(code);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };
  return (
    <div className={styles.docCodeWrapper}>
      <div className={styles.docCodeHeader}>
        <div style={{ display: "flex", gap: "6px", alignItems: "center" }}>
          <span style={{ width: "9px", height: "9px", borderRadius: "50%", background: "#ff5f56" }} />
          <span style={{ width: "9px", height: "9px", borderRadius: "50%", background: "#ffbd2e" }} />
          <span style={{ width: "9px", height: "9px", borderRadius: "50%", background: "#27c93f" }} />
        </div>
        <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: "11px", color: "#6b7080" }}>Terminal / Config</span>
        <button className={styles.docCodeCopy} onClick={handleCopy} type="button">
          {copied ? "Copied ✓" : "Copy"}
        </button>
      </div>
      <pre className={styles.docCode}><code>{code}</code></pre>
    </div>
  );
}

function DocBlockView({ block }: { block: DocBlock }) {
  if ("h" in block) return <h3 className={styles.docHeading}>{block.h}</h3>;
  if ("p" in block) return <p className={styles.docPara}>{block.p}</p>;
  if ("c" in block) return <DocCode code={block.c} />;
  if ("l" in block) {
    return (
      <ul className={styles.docList}>
        {block.l.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
    );
  }
  return (
    <div className={styles.cmdTable}>
      {block.cmds.map(([cmd, desc]) => (
        <div key={cmd}>
          <code>{cmd}</code>
          <span>{desc}</span>
        </div>
      ))}
    </div>
  );
}
