'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Bookmark as BookmarkIcon, Camera, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';

interface Bookmark {
  id: string;
  meeting_id: string;
  label?: string | null;
  offset_seconds?: number | null;
  image_path?: string | null;
  created_at: string;
}

interface MeetingBookmarksProps {
  /** Persisted meeting id; when absent we're live-recording and bookmarks queue. */
  meetingId?: string | null;
  /** Show the "Bookmark" / "Capture slide" buttons (e.g. while recording). */
  canAdd?: boolean;
  /** Optional seek callback for audio playback (receives offset_seconds). */
  onSeekTo?: (offsetSeconds: number) => void;
}

function fmtOffset(sec?: number | null): string {
  if (sec == null) return '—';
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
}

/**
 * In-meeting bookmarks + slide captures. While recording (no meeting id),
 * bookmarks queue in the backend pending list; once `meetingId` appears the
 * component finalizes them into SQLite.
 */
export function MeetingBookmarks({ meetingId, canAdd = false, onSeekTo }: MeetingBookmarksProps) {
  const [bookmarks, setBookmarks] = useState<Bookmark[]>([]);

  const refresh = useCallback(async () => {
    try {
      if (meetingId) {
        setBookmarks(await invoke<Bookmark[]>('list_bookmarks', { meetingId }));
      } else {
        setBookmarks(await invoke<Bookmark[]>('list_pending_bookmarks'));
      }
    } catch (e) {
      console.error('list bookmarks failed', e);
    }
  }, [meetingId]);

  // Once a real meeting id exists, flush the pending live-recording queue.
  useEffect(() => {
    if (!meetingId) {
      refresh();
      return;
    }
    (async () => {
      try {
        await invoke<Bookmark[]>('finalize_pending_bookmarks', { meetingId });
      } catch (e) {
        console.error('finalize pending bookmarks failed', e);
      }
      refresh();
    })();
  }, [meetingId, refresh]);

  const add = async () => {
    try {
      await invoke('add_bookmark', { meetingId: meetingId ?? null, label: null, offsetSeconds: null });
      refresh();
    } catch (e) {
      toast.error(`Bookmark failed: ${e}`);
    }
  };

  const captureSlide = async () => {
    try {
      await invoke('capture_frame_note', { meetingId: meetingId ?? null, label: null });
      toast.success('Slide captured');
      refresh();
    } catch (e) {
      toast.error(`Slide capture failed: ${e}`);
    }
  };

  const remove = async (id: string) => {
    try {
      await invoke('delete_bookmark', { id });
      refresh();
    } catch (e) {
      toast.error(`Delete failed: ${e}`);
    }
  };

  if (!canAdd && bookmarks.length === 0) return null;

  return (
    <div className="px-4 py-2 border-b border-gray-200 bg-white">
      <div className="flex items-center gap-2 flex-wrap">
        {canAdd && (
          <>
            <Button variant="outline" size="sm" onClick={add} className="h-7 text-xs">
              <BookmarkIcon className="w-3.5 h-3.5 mr-1" /> Bookmark
            </Button>
            <Button variant="outline" size="sm" onClick={captureSlide} className="h-7 text-xs">
              <Camera className="w-3.5 h-3.5 mr-1" /> Capture slide
            </Button>
          </>
        )}
        {bookmarks.map(b => (
          <span key={b.id} className="inline-flex items-center gap-1 text-xs bg-gray-100 rounded px-2 py-1">
            <button
              className="text-blue-600 hover:underline disabled:text-gray-600 disabled:no-underline"
              disabled={!onSeekTo || b.offset_seconds == null}
              onClick={() => b.offset_seconds != null && onSeekTo?.(b.offset_seconds)}
              title={b.image_path ? `Slide: ${b.image_path}` : undefined}
            >
              {fmtOffset(b.offset_seconds)}
            </button>
            {b.image_path && <Camera className="w-3 h-3 text-gray-500" />}
            {b.label && <span className="text-gray-600">{b.label}</span>}
            <button onClick={() => remove(b.id)} className="text-gray-400 hover:text-red-500">
              <Trash2 className="w-3 h-3" />
            </button>
          </span>
        ))}
      </div>
    </div>
  );
}
