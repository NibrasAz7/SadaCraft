'use client';

import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Switch } from '@/components/ui/switch';
import { Button } from '@/components/ui/button';
import { ConfirmationModal } from '@/components/ConfirmationModel/confirmation-modal';
import { Trash2, Loader2 } from 'lucide-react';

interface Lesson {
  rule: string;
  wins: number;
}

type LessonsMap = Record<string, Lesson[]>;

interface TemplateInfo {
  id: string;
  name: string;
  description: string;
}

export function SummarySelfImproveSettings() {
  const [enabled, setEnabled] = useState(false);
  const [saving, setSaving] = useState(false);
  const [lessons, setLessons] = useState<LessonsMap>({});
  const [templateNames, setTemplateNames] = useState<Record<string, string>>({});
  const [lessonsLoading, setLessonsLoading] = useState(true);
  const [pendingClear, setPendingClear] = useState<string | null>(null);

  const loadLessons = useCallback(async () => {
    try {
      const data = await invoke<LessonsMap>('api_list_summary_lessons');
      setLessons(data);
    } catch (error) {
      console.error('Failed to load summary lessons:', error);
    } finally {
      setLessonsLoading(false);
    }
  }, []);

  useEffect(() => {
    const loadSetting = async () => {
      try {
        const value = await invoke<boolean>('api_get_summary_self_improve_enabled');
        setEnabled(value);
      } catch (error) {
        console.error('Failed to load self-improvement setting:', error);
      }
    };
    loadSetting();
    loadLessons();

    const loadTemplates = async () => {
      try {
        const templates = await invoke<TemplateInfo[]>('api_list_templates');
        const names: Record<string, string> = {};
        for (const t of templates) {
          names[t.id] = t.name;
        }
        setTemplateNames(names);
      } catch (error) {
        console.error('Failed to load templates:', error);
      }
    };
    loadTemplates();
  }, [loadLessons]);

  const handleToggle = async (value: boolean) => {
    setEnabled(value);
    setSaving(true);
    try {
      await invoke('api_set_summary_self_improve_enabled', { enabled: value });
      toast.success(value ? 'Summary self-improvement enabled' : 'Summary self-improvement disabled');
    } catch (error) {
      console.error('Failed to save self-improvement setting:', error);
      setEnabled(!value);
      toast.error('Failed to save setting');
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteLesson = async (templateId: string, rule: string) => {
    try {
      await invoke('api_delete_summary_lesson', { templateId, rule });
      setLessons(prev => {
        const next = { ...prev };
        const remaining = (next[templateId] ?? []).filter(l => l.rule !== rule);
        if (remaining.length === 0) {
          delete next[templateId];
        } else {
          next[templateId] = remaining;
        }
        return next;
      });
      toast.success('Rule deleted');
    } catch (error) {
      console.error('Failed to delete lesson:', error);
      toast.error('Failed to delete rule');
    }
  };

  const handleClearTemplate = async (templateId: string) => {
    try {
      await invoke('api_clear_summary_lessons', { templateId });
      setLessons(prev => {
        const next = { ...prev };
        delete next[templateId];
        return next;
      });
      toast.success('All rules cleared');
    } catch (error) {
      console.error('Failed to clear lessons:', error);
      toast.error('Failed to clear rules');
    } finally {
      setPendingClear(null);
    }
  };

  const templateIds = Object.keys(lessons);

  return (
    <>
      {/* Self-Improvement Toggle */}
      <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-lg font-semibold text-gray-900 mb-2">Self-Improving Summaries</h3>
            <p className="text-sm text-gray-600">
              After each summary, a local improvement round proposes a refinement, tests it, and
              keeps it only if it wins. Learned rules are applied to future summaries.
            </p>
          </div>
          <div className="flex items-center gap-2 ml-4">
            {saving && <Loader2 className="w-4 h-4 animate-spin text-gray-500" />}
            <Switch checked={enabled} onCheckedChange={handleToggle} disabled={saving} />
          </div>
        </div>
      </div>

      {/* Learned Rules */}
      <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
        <h3 className="text-lg font-semibold text-gray-900 mb-2">Learned Summary Rules</h3>
        <p className="text-sm text-gray-600 mb-6">
          Rules the model has learned from past summaries. They are injected into every future
          summary for that template.
        </p>

        {lessonsLoading ? (
          <div className="flex items-center gap-2 text-sm text-gray-500">
            <Loader2 className="w-4 h-4 animate-spin" />
            Loading rules...
          </div>
        ) : templateIds.length === 0 ? (
          <div className="p-4 border rounded-lg bg-gray-50 text-sm text-gray-600">
            No learned rules yet. They will appear here as the self-improvement pass runs.
          </div>
        ) : (
          <div className="space-y-4">
            {templateIds.map(templateId => (
              <div key={templateId} className="border rounded-lg">
                <div className="flex items-center justify-between px-4 py-3 border-b bg-gray-50">
                  <div className="font-medium text-gray-900">
                    {templateNames[templateId] ?? templateId}
                    {templateNames[templateId] && (
                      <span className="ml-2 text-xs text-gray-500 font-normal">{templateId}</span>
                    )}
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setPendingClear(templateId)}
                    className="text-red-600 hover:text-red-700 hover:bg-red-50"
                  >
                    Clear all
                  </Button>
                </div>
                <ul className="divide-y">
                  {lessons[templateId].map(lesson => (
                    <li key={lesson.rule} className="flex items-center gap-3 px-4 py-3">
                      <span className="flex-1 text-sm text-gray-800">{lesson.rule}</span>
                      <span className="text-xs text-gray-500 whitespace-nowrap">
                        {lesson.wins} {lesson.wins === 1 ? 'win' : 'wins'}
                      </span>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => handleDeleteLesson(templateId, lesson.rule)}
                        className="h-8 w-8 text-gray-400 hover:text-red-600 hover:bg-red-50"
                        title="Delete rule"
                      >
                        <Trash2 className="w-4 h-4" />
                      </Button>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        )}
      </div>

      <ConfirmationModal
        isOpen={pendingClear !== null}
        onCancel={() => setPendingClear(null)}
        onConfirm={() => pendingClear && handleClearTemplate(pendingClear)}
        text={
          pendingClear
            ? `Delete all learned rules for "${templateNames[pendingClear] ?? pendingClear}"?`
            : ''
        }
      />
    </>
  );
}
