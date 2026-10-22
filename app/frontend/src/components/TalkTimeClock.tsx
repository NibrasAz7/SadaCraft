'use client';

import { useMemo } from 'react';
import { TranscriptSegmentData } from '@/types';

const SPEAKER_COLORS: Record<string, string> = {
  You: 'bg-blue-500',
  Remote: 'bg-emerald-500',
};

const PALETTE = ['bg-violet-500', 'bg-amber-500', 'bg-rose-500', 'bg-cyan-500', 'bg-lime-600'];

function colorFor(speaker: string, remoteIdx: number): string {
  if (SPEAKER_COLORS[speaker]) return SPEAKER_COLORS[speaker];
  return PALETTE[remoteIdx % PALETTE.length];
}

function fmt(sec: number): string {
  const m = Math.floor(sec / 60);
  const s = Math.round(sec % 60);
  return `${m}m ${s}s`;
}

interface TalkTimeClockProps {
  /** Transcript segments; talk time derives from timestamp→endTime durations. */
  segments: TranscriptSegmentData[];
}

/**
 * Per-speaker talk-time share bar. Renders nothing when fewer than two
 * speakers are present or no durations exist.
 */
export function TalkTimeClock({ segments }: TalkTimeClockProps) {
  const stats = useMemo(() => {
    const acc = new Map<string, number>();
    for (const s of segments) {
      const dur = (s.endTime ?? s.timestamp + 2) - s.timestamp;
      if (dur <= 0) continue;
      const who = s.speaker || 'Remote';
      acc.set(who, (acc.get(who) ?? 0) + dur);
    }
    const total = [...acc.values()].reduce((a, b) => a + b, 0);
    if (total <= 0 || acc.size < 2) return null;
    const entries = [...acc.entries()].sort((a, b) => b[1] - a[1]);
    return { total, entries };
  }, [segments]);

  if (!stats) return null;

  let remoteIdx = 0;
  const parts = stats.entries.map(([who, sec]) => {
    const remoteLike = who === 'Remote' || who.startsWith('Remote-');
    const color = remoteLike && who !== 'Remote' ? colorFor(who, remoteIdx++) : (SPEAKER_COLORS[who] ?? PALETTE[remoteIdx++ % PALETTE.length]);
    return { who, sec, pct: (sec / stats.total) * 100, color };
  });

  return (
    <div className="px-4 py-2 border-b border-gray-200 bg-white">
      <div className="flex h-2 rounded-full overflow-hidden bg-gray-100">
        {parts.map(p => (
          <div key={p.who} className={p.color} style={{ width: `${p.pct}%` }} title={`${p.who}: ${fmt(p.sec)}`} />
        ))}
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-0.5 mt-1.5">
        {parts.map(p => (
          <span key={p.who} className="inline-flex items-center gap-1.5 text-xs text-gray-600">
            <span className={`w-2 h-2 rounded-full ${p.color}`} />
            {p.who} {fmt(p.sec)} · {Math.round(p.pct)}%
          </span>
        ))}
      </div>
    </div>
  );
}
