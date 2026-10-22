'use client';

import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Switch } from '@/components/ui/switch';
import { Loader2 } from 'lucide-react';

/**
 * Settings card for the `meeting_autodetect` toggle persisted in store.json.
 * Mount inside `PreferenceSettings` (Settings → General) or
 * `RecordingSettings` (Settings → Recordings) — wherever the product wants it;
 * this file only owns the card itself.
 */
export function MeetingAutodetectSettings() {
  const [enabled, setEnabled] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const loadSetting = async () => {
      try {
        const value = await invoke<boolean>('api_get_meeting_autodetect_enabled');
        setEnabled(value);
      } catch (error) {
        console.error('Failed to load meeting auto-detect setting:', error);
      }
    };
    loadSetting();
  }, []);

  const handleToggle = async (value: boolean) => {
    setEnabled(value);
    setSaving(true);
    try {
      await invoke('api_set_meeting_autodetect_enabled', { enabled: value });
      toast.success(
        value ? 'Meeting auto-detect enabled' : 'Meeting auto-detect disabled'
      );
    } catch (error) {
      console.error('Failed to save meeting auto-detect setting:', error);
      setEnabled(!value);
      toast.error('Failed to save setting');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-2">
            Meeting Auto-Detect
          </h3>
          <p className="text-sm text-gray-600">
            Notify when a meeting app (Zoom, Teams, WebEx) or sustained
            conversation audio is detected, with a one-click option to start
            recording. While enabled, a low-level system-audio monitor stays
            open. Detection runs fully on-device.
          </p>
        </div>
        <div className="flex items-center gap-2 ml-4">
          {saving && <Loader2 className="w-4 h-4 animate-spin text-gray-500" />}
          <Switch checked={enabled} onCheckedChange={handleToggle} disabled={saving} />
        </div>
      </div>
    </div>
  );
}
