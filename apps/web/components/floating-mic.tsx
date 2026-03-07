"use client";

import { useState, useEffect } from "react";
import { MicIcon } from "./icons";

export function FloatingMic() {
  const [isActive, setIsActive] = useState(false);

  useEffect(() => {
    const interval = setInterval(() => {
      setIsActive(true);
      setTimeout(() => setIsActive(false), 3000);
    }, 5000);
    const initialTimeout = setTimeout(() => setIsActive(true), 800);
    return () => {
      clearInterval(interval);
      clearTimeout(initialTimeout);
    };
  }, []);

  return (
    <div className="relative flex items-center justify-center">
      {/* Outer glow rings */}
      {isActive && (
        <>
          <div className="absolute h-24 w-24 rounded-full bg-accent/20 animate-pulse-ring" />
          <div
            className="absolute h-32 w-32 rounded-full bg-accent/10 animate-pulse-ring"
            style={{ animationDelay: "0.5s" }}
          />
        </>
      )}

      {/* Main mic orb */}
      <div
        className={`
          relative z-10 flex h-16 w-16 items-center justify-center
          rounded-full shadow-2xl transition-all duration-500 animate-float
          ${
            isActive
              ? "bg-accent shadow-accent/40 scale-110"
              : "bg-bg-elevated shadow-black/40"
          }
        `}
      >
        <MicIcon
          className={`h-7 w-7 transition-colors duration-500 ${
            isActive ? "text-white" : "text-text-muted"
          }`}
        />
      </div>
    </div>
  );
}
