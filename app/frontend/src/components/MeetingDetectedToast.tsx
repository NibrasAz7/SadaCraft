'use client';

import React, { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { usePathname, useRouter } from 'next/navigation';

interface MeetingDetectedPayload {
  /** 'process' | 'audio' */
  source: string;
  /** Rough 0..1 heuristic confidence. */
  confidence: number;
  /** Human-readable detail, e.g. the process name or threshold hit. */
  detail?: string;
}

/**
 * Listens for the backend `meeting-detected` event and offers to start
 * recording. Mount inside SidebarProvider in `app/layout.tsx`, alongside
 * `<DownloadProgressToastProvider />` — it only renders a toast, no DOM.
 *
 * The backend already debounces to one event per meeting episode and
 * suppresses events while a recording is active.
 */
export function MeetingDetectedToastProvider() {
  const router = useRouter();
  const pathname = usePathname();

  useEffect(() => {
    const unlisten = listen<MeetingDetectedPayload>('meeting-detected', (event) => {
      const { source, confidence, detail } = event.payload;
      const pct = Math.round(confidence * 100);
      const reason =
        source === 'process'
          ? `${detail ?? 'A meeting app'} is running`
          : detail ?? 'Speech-level system audio detected';

      toast('Meeting detected', {
        description: `${reason}. Start recording?`,
        duration: 15000,
        action: {
          label: `Start Recording (${pct}%)`,
          onClick: () => {
            if (pathname === '/') {
              // Home already listens for this event (useRecordingStart).
              window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
            } else {
              // Same mechanism the sidebar uses when off the home page.
              sessionStorage.setItem('autoStartRecording', 'true');
              router.push('/');
            }
          },
        },
      });
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [pathname, router]);

  return null;
}
