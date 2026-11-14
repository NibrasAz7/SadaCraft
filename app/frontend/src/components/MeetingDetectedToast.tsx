'use client';

import React, { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { usePathname, useRouter } from 'next/navigation';

interface MeetingDetectedPayload {
  /** 'process' | 'audio' */
  source: string;
  /** Rough 0..1 heuristic confidence. */
  confidence: number;
  /** Human-readable detail, e.g. the process name or threshold hit. */
  detail?: string;
  /** Display name of the detected meeting process (process heuristic only). */
  process_name?: string;
  /** PID of the detected meeting process — enables one-click per-app capture. */
  pid?: number;
}

/** Must match PENDING_MEETING_CAPTURE_KEY in useRecordingStart.ts. */
const PENDING_MEETING_CAPTURE_KEY = 'sadacraft_pending_meeting_capture';

/**
 * Listens for the backend `meeting-detected` event and offers to start
 * recording. Mount inside SidebarProvider in `app/layout.tsx`, alongside
 * `<DownloadProgressToastProvider />` — it only renders a toast, no DOM.
 *
 * The backend already debounces to one event per meeting episode and
 * suppresses events while a recording is active.
 *
 * When the detection carries a PID and per-app capture is supported
 * (Windows 10 build 20348+), the one-click action records mic + that app's
 * process-loopback audio via a one-shot sessionStorage override consumed by
 * useRecordingStart. Otherwise it falls back to the normal device selection
 * (mic + full system loopback).
 */
export function MeetingDetectedToastProvider() {
  const router = useRouter();
  const pathname = usePathname();

  useEffect(() => {
    const unlisten = listen<MeetingDetectedPayload>('meeting-detected', (event) => {
      const { source, confidence, detail, process_name, pid } = event.payload;
      const pct = Math.round(confidence * 100);
      const appLabel = process_name ?? detail ?? 'A meeting app';
      const reason =
        source === 'process'
          ? `${appLabel} is running`
          : detail ?? 'Speech-level system audio detected';

      toast('Meeting detected', {
        description: `${reason}. Start recording?`,
        duration: 15000,
        action: {
          label: `Start Recording (${pct}%)`,
          onClick: async () => {
            // Per-app capture: only when we have a PID and the OS supports
            // WASAPI process loopback. Awaited so the pending override is in
            // sessionStorage before the start path reads it. The override is
            // consumed by the next recording start
            // (useRecordingStart.buildRecordingArgs).
            if (typeof pid === 'number' && pid > 0) {
              try {
                const supported = await invoke<boolean>('audio_app_capture_supported');
                if (supported) {
                  sessionStorage.setItem(
                    PENDING_MEETING_CAPTURE_KEY,
                    JSON.stringify({ pid, appName: process_name ?? detail ?? null })
                  );
                }
              } catch {
                // Command unavailable (non-Windows build) — normal start.
              }
            }

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
