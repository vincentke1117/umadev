"use client";

import { ErrorFallback } from "./ErrorFallback";

export default function ErrorPage({
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  return <ErrorFallback reset={reset} />;
}
