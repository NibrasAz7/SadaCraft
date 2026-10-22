'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Check, Loader2, Pencil, Users, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { TranscriptSegmentData } from '@/types';

/** Per-speaker stats returned by the `api_get_meeting_speakers` command */
interface SpeakerStat {
  speaker: string;
  line_count: number;
  talk_time_seconds: number;
}

interface SpeakerPanelProps {
  /** Meeting id. When set, stats come from SQLite and renames are persisted via `rename_speaker`. */
  meetingId?: string | null;
  /** Already-loaded transcript segments, used to derive stats when `meetingId` is absent. */
  segments?: TranscriptSegmentData[];
  /** Called after a rename succeeds so parents can refresh displayed transcript labels. */
  onSpeakerRenamed?: (oldLabel: string, newLabel: string) => void;
  className?: string;
}

/** Same chip colors as VirtualizedTranscriptView: "You" is blue, everyone else emerald. */
function speakerChipClass(speaker: string): string {
  return speaker === 'You'
    ? 'bg-blue-100 text-blue-700'
    : 'bg-emerald-100 text-emerald-700';
}

function formatTalkTime(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${secs}s`;
  return `${secs}s`;
}

/** Merge `labelOverrides` (pending/confirmed renames) into a stats list. */
function applyOverrides(stats: SpeakerStat[], overrides: Record<string, string>): SpeakerStat[] {
  if (Object.keys(overrides).length === 0) return stats;
  const merged = new Map<string, SpeakerStat>();
  for (const stat of stats) {
    const label = overrides[stat.speaker] ?? stat.speaker;
    const entry = merged.get(label) ?? { speaker: label, line_count: 0, talk_time_seconds: 0 };
    entry.line_count += stat.line_count;
    entry.talk_time_seconds += stat.talk_time_seconds;
    merged.set(label, entry);
  }
  return [...merged.values()].sort((a, b) => b.talk_time_seconds - a.talk_time_seconds);
}

export function SpeakerPanel({
  meetingId,
  segments,
  onSpeakerRenamed,
  className = '',
}: SpeakerPanelProps) {
  const [fetchedStats, setFetchedStats] = useState<SpeakerStat[]>([]);
  const [overrides, setOverrides] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [editValue, setEditValue] = useState('');
  const [saving, setSaving] = useState(false);

  // Persisted path: aggregate stats in SQLite so they cover the full meeting
  // even when transcripts are paginated on the frontend.
  const loadStats = useCallback(async () => {
    if (!meetingId) return;
    setLoading(true);
    try {
      const stats = await invoke<SpeakerStat[]>('api_get_meeting_speakers', { meetingId });
      setFetchedStats(stats);
    } catch (error) {
      console.error('Failed to load meeting speakers:', error);
    } finally {
      setLoading(false);
    }
  }, [meetingId]);

  useEffect(() => {
    setOverrides({});
    setEditing(null);
    setFetchedStats([]);
    void loadStats();
  }, [loadStats]);

  // Live path: derive stats from the segments prop when no meeting is persisted.
  const derivedStats = useMemo(() => {
    const map = new Map<string, SpeakerStat>();
    for (const segment of segments ?? []) {
      if (!segment.speaker) continue;
      const entry = map.get(segment.speaker) ?? {
        speaker: segment.speaker,
        line_count: 0,
        talk_time_seconds: 0,
      };
      entry.line_count += 1;
      entry.talk_time_seconds += Math.max(0, (segment.endTime ?? segment.timestamp) - segment.timestamp);
      map.set(segment.speaker, entry);
    }
    return [...map.values()].sort((a, b) => b.talk_time_seconds - a.talk_time_seconds);
  }, [segments]);

  const stats = useMemo(
    () => applyOverrides(meetingId ? fetchedStats : derivedStats, overrides),
    [meetingId, fetchedStats, derivedStats, overrides]
  );

  const startRename = (label: string) => {
    setEditing(label);
    setEditValue(label);
  };

  const cancelRename = () => {
    setEditing(null);
    setEditValue('');
  };

  const commitRename = async () => {
    if (!editing) return;
    const newLabel = editValue.trim();
    if (!newLabel || newLabel === editing) {
      cancelRename();
      return;
    }

    setSaving(true);
    try {
      if (meetingId) {
        await invoke<number>('rename_speaker', {
          meetingId,
          oldLabel: editing,
          newLabel,
        });
      }
      setOverrides(prev => ({ ...prev, [editing]: newLabel }));
      onSpeakerRenamed?.(editing, newLabel);
      toast.success(`Renamed "${editing}" to "${newLabel}"`);
      if (meetingId) {
        await loadStats();
      }
    } catch (error) {
      console.error('Failed to rename speaker:', error);
      toast.error('Failed to rename speaker', { description: String(error) });
    } finally {
      setSaving(false);
      setEditing(null);
      setEditValue('');
    }
  };

  return (
    <div className={`bg-white rounded-lg border border-gray-200 p-4 ${className}`}>
      <div className="flex items-center gap-2 mb-3">
        <Users className="w-4 h-4 text-gray-500" />
        <h3 className="text-sm font-semibold text-gray-900">Speakers</h3>
      </div>

      {loading && stats.length === 0 ? (
        <div className="flex items-center gap-2 text-sm text-gray-500">
          <Loader2 className="w-4 h-4 animate-spin" />
          Loading speakers...
        </div>
      ) : stats.length === 0 ? (
        <p className="text-sm text-gray-500">No speakers identified yet.</p>
      ) : (
        <ul className="space-y-1">
          {stats.map(stat => (
            <li key={stat.speaker} className="flex items-center gap-2 py-1">
              {editing === stat.speaker ? (
                <>
                  <input
                    autoFocus
                    value={editValue}
                    onChange={e => setEditValue(e.target.value)}
                    onKeyDown={e => {
                      if (e.key === 'Enter') void commitRename();
                      if (e.key === 'Escape') cancelRename();
                    }}
                    maxLength={64}
                    disabled={saving}
                    className="flex-1 min-w-0 px-2 py-1 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500"
                  />
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => void commitRename()}
                    disabled={saving}
                    className="h-7 w-7 text-gray-500 hover:text-green-600"
                    title="Save name"
                  >
                    {saving ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Check className="w-3.5 h-3.5" />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={cancelRename}
                    disabled={saving}
                    className="h-7 w-7 text-gray-500 hover:text-red-600"
                    title="Cancel"
                  >
                    <X className="w-3.5 h-3.5" />
                  </Button>
                </>
              ) : (
                <>
                  <span
                    className={`text-[10px] font-medium px-1.5 py-0.5 rounded flex-shrink-0 ${speakerChipClass(stat.speaker)}`}
                  >
                    {stat.speaker}
                  </span>
                  <span className="flex-1 min-w-0 text-xs text-gray-500 truncate">
                    {stat.line_count} {stat.line_count === 1 ? 'line' : 'lines'}
                    {' · '}
                    {formatTalkTime(stat.talk_time_seconds)}
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => startRename(stat.speaker)}
                    className="h-7 w-7 text-gray-400 hover:text-gray-700"
                    title={`Rename ${stat.speaker}`}
                  >
                    <Pencil className="w-3.5 h-3.5" />
                  </Button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
