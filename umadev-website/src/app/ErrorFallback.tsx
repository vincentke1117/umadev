"use client";

import styles from "./error.module.css";

const copy = {
  eyebrow: "UMADEV / RECOVERY",
  title: "页面暂时无法显示 / Page unavailable",
  descriptionZh: "页面遇到了未预期的问题。你可以重试当前操作，或返回首页。",
  descriptionEn: "Something unexpected happened. Retry this view or return home.",
  retry: "重试 / Retry",
  home: "返回首页 / Home",
} as const;

export function ErrorFallback({ reset }: { reset: () => void }) {
  const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? "";

  return (
    <main className={styles.shell} role="alert" aria-live="assertive">
      <section className={styles.panel}>
        <p className={styles.eyebrow}>{copy.eyebrow}</p>
        <h1>{copy.title}</h1>
        <p className={styles.description}>
          {copy.descriptionZh}
          <br />
          {copy.descriptionEn}
        </p>
        <div className={styles.actions}>
          <button aria-label={copy.retry} className={styles.retry} type="button" onClick={reset}>
            {copy.retry}
          </button>
          <a className={styles.home} href={`${basePath}/`}>
            {copy.home}
          </a>
        </div>
      </section>
    </main>
  );
}
