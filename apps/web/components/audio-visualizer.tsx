"use client";

import { Visualizer } from "react-sound-visualizer";

interface AudioVisualizerProps {
  audio: MediaStream | null;
  className?: string;
  width?: number;
  height?: number;
}

export function AudioVisualizer({
  audio,
  className = "",
  width = 48,
  height = 28,
}: AudioVisualizerProps) {
  if (!audio) return null;

  return (
    <div className={`relative ${className}`}>
      {/* Idle baseline — visible at rest */}
      <div
        className="absolute inset-0 flex items-center justify-center pointer-events-none"
        aria-hidden
      >
        <div className="w-full h-0.5 rounded-full bg-[#9ef0c9]/60" />
      </div>
      <Visualizer
        audio={audio}
        strokeColor="#9ef0c9"
        autoStart
        rectWidth={3}
        slices={20}
      >
        {({ canvasRef }) => (
          <canvas
            ref={canvasRef}
            width={width}
            height={height}
            className="relative block w-full h-full"
            style={{ minWidth: width, minHeight: height }}
          />
        )}
      </Visualizer>
    </div>
  );
}
