"use client";

import { useState, useEffect, useRef } from "react";

const WAVE_HEIGHTS = [4, 8, 11, 7, 10, 5, 8];
const IDLE_DOTS = [0, 1, 2];

export function FloatingMic() {
  const [isActive, setIsActive] = useState(false);
  const [isHovered, setIsHovered] = useState(false);
  const [showTooltip, setShowTooltip] = useState(false);
  const tooltipTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const initialTimeout = setTimeout(() => setIsActive(true), 800);
    const interval = setInterval(() => {
      setIsActive(true);
      setTimeout(() => setIsActive(false), 3000);
    }, 5000);
    return () => {
      clearTimeout(initialTimeout);
      clearInterval(interval);
    };
  }, []);

  const handleMouseEnter = () => {
    setIsHovered(true);
    tooltipTimer.current = setTimeout(() => setShowTooltip(true), 120);
  };

  const handleMouseLeave = () => {
    setIsHovered(false);
    setShowTooltip(false);
    if (tooltipTimer.current) clearTimeout(tooltipTimer.current);
  };

  return (
    <div
      className="relative flex items-center justify-center"
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      {/* Tooltip */}
      <div
        className={`
          pointer-events-none absolute -top-10 left-1/2 -translate-x-1/2
          whitespace-nowrap rounded-lg border border-border bg-bg-elevated px-3 py-1.5
          text-xs text-text-muted shadow-lg transition-all duration-150
          ${showTooltip && !isActive ? "opacity-100 -translate-y-1" : "opacity-0 translate-y-0"}
        `}
      >
        Hold <kbd className="font-mono font-semibold text-text">Ctrl+Space</kbd> to record
      </div>

      {isActive && (
        <div className="absolute inset-0 -m-3 rounded-full bg-accent/15 animate-pulse-ring" />
      )}

      <div
        className={`
          relative z-10 flex animate-float items-center gap-2
          rounded-full shadow-xl transition-all duration-300
          ${
            isActive
              ? "bg-accent shadow-accent/30 scale-105 px-4 py-2.5"
              : isHovered
                ? "border border-border/80 bg-bg-elevated shadow-black/30 scale-105 px-4 py-2.5"
                : "border border-border/50 bg-bg-elevated/80 shadow-black/20 px-2.5 py-2"
          }
        `}
      >
        <svg
          className={`flex-shrink-0 transition-all duration-300 ${
            isActive
              ? "h-4 w-4 text-white"
              : isHovered
                ? "h-4 w-4 text-text-muted"
                : "h-3.5 w-3.5 text-text-muted/60"
          }`}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={1.5}
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
          <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
          <line x1="12" x2="12" y1="19" y2="22" />
        </svg>

        {isActive ? (
          <div className="flex items-center gap-[3px]">
            {WAVE_HEIGHTS.map((h, i) => (
              <div
                key={i}
                className="w-[3px] rounded-full bg-white/80 animate-wave-bar"
                style={{ height: `${h}px`, animationDelay: `${i * 0.09}s` }}
              />
            ))}
          </div>
        ) : isHovered ? (
          <div className="flex items-center gap-[5px]">
            {IDLE_DOTS.map((i) => (
              <div
                key={i}
                className="h-1.5 w-1.5 rounded-full bg-white/40 animate-idle-dot"
                style={{ animationDelay: `${i * 0.18}s` }}
              />
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}
