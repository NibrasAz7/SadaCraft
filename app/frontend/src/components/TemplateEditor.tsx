'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { ConfirmationModal } from '@/components/ConfirmationModel/confirmation-modal';
import { ChevronDown, ChevronUp, Loader2, Plus, Trash2 } from 'lucide-react';

interface TemplateInfo {
  id: string;
  name: string;
  description: string;
  is_custom: boolean;
}

interface TemplateSection {
  title: string;
  instruction: string;
  format: string;
  item_format?: string;
  example_item_format?: string;
}

interface TemplateContent {
  id: string;
  name: string;
  description: string;
  sections: TemplateSection[];
  is_custom: boolean;
}

interface TemplateDraft {
  name: string;
  description: string;
  sections: TemplateSection[];
}

const SECTION_FORMATS = ['paragraph', 'list', 'string'] as const;

const EMPTY_SECTION: TemplateSection = {
  title: '',
  instruction: '',
  format: 'paragraph',
};

function emptyDraft(): TemplateDraft {
  return { name: '', description: '', sections: [{ ...EMPTY_SECTION }] };
}

/** Suggested id for "save as new": `custom-` + slugified template name. */
function suggestedTemplateId(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return `custom-${slug || 'template'}`;
}

/** Builds the JSON payload the backend validates against the summary template schema. */
function draftToPayload(draft: TemplateDraft) {
  return {
    name: draft.name.trim(),
    description: draft.description.trim(),
    sections: draft.sections.map(section => ({
      title: section.title.trim(),
      instruction: section.instruction.trim(),
      format: section.format,
      ...(section.item_format?.trim() ? { item_format: section.item_format } : {}),
      ...(section.example_item_format?.trim()
        ? { example_item_format: section.example_item_format }
        : {}),
    })),
  };
}

function validateDraft(draft: TemplateDraft): string | null {
  if (!draft.name.trim()) return 'Template name cannot be empty';
  if (!draft.description.trim()) return 'Template description cannot be empty';
  if (draft.sections.length === 0) return 'Template must have at least one section';
  for (const section of draft.sections) {
    if (!section.title.trim()) return 'Every section needs a title';
    if (!section.instruction.trim()) {
      return `Section "${section.title.trim() || 'Untitled'}" needs an instruction`;
    }
  }
  return null;
}

export function TemplateEditor() {
  const [templates, setTemplates] = useState<TemplateInfo[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<TemplateDraft>(emptyDraft());
  const [isCustom, setIsCustom] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveAsOpen, setSaveAsOpen] = useState(false);
  const [saveAsId, setSaveAsId] = useState('');
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);

  const loadTemplates = useCallback(async (): Promise<TemplateInfo[]> => {
    try {
      const list = await invoke<TemplateInfo[]>('api_list_templates');
      setTemplates(list);
      return list;
    } catch (error) {
      console.error('Failed to load templates:', error);
      return [];
    }
  }, []);

  const loadTemplate = useCallback(async (templateId: string) => {
    setLoading(true);
    try {
      const content = await invoke<TemplateContent>('api_get_template_content', { templateId });
      setDraft({
        name: content.name,
        description: content.description,
        sections: content.sections.map(section => ({ ...section })),
      });
      setIsCustom(content.is_custom);
    } catch (error) {
      console.error('Failed to load template:', error);
      toast.error('Failed to load template', { description: String(error) });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadTemplates();
  }, [loadTemplates]);

  const handleSelectTemplate = (templateId: string) => {
    setSelectedId(templateId);
    setSaveAsOpen(false);
    void loadTemplate(templateId);
  };

  const handleNewTemplate = () => {
    setSelectedId(null);
    setDraft(emptyDraft());
    setIsCustom(true);
    setSaveAsId(suggestedTemplateId(''));
    setSaveAsOpen(true);
  };

  const updateSection = (index: number, patch: Partial<TemplateSection>) => {
    setDraft(prev => ({
      ...prev,
      sections: prev.sections.map((s, i) => (i === index ? { ...s, ...patch } : s)),
    }));
  };

  const moveSection = (index: number, direction: -1 | 1) => {
    setDraft(prev => {
      const target = index + direction;
      if (target < 0 || target >= prev.sections.length) return prev;
      const sections = [...prev.sections];
      [sections[index], sections[target]] = [sections[target], sections[index]];
      return { ...prev, sections };
    });
  };

  const removeSection = (index: number) => {
    setDraft(prev => ({
      ...prev,
      sections: prev.sections.filter((_, i) => i !== index),
    }));
  };

  const addSection = () => {
    setDraft(prev => ({ ...prev, sections: [...prev.sections, { ...EMPTY_SECTION }] }));
  };

  const handleSave = async () => {
    if (!selectedId) return;
    const validationError = validateDraft(draft);
    if (validationError) {
      toast.error(validationError);
      return;
    }

    setSaving(true);
    try {
      await invoke<TemplateInfo>('api_update_template', {
        templateId: selectedId,
        templateJson: JSON.stringify(draftToPayload(draft)),
      });
      setIsCustom(true);
      await loadTemplates();
      toast.success('Template saved');
    } catch (error) {
      console.error('Failed to save template:', error);
      toast.error('Failed to save template', { description: String(error) });
    } finally {
      setSaving(false);
    }
  };

  const handleSaveAsNew = async () => {
    const templateId = saveAsId.trim();
    if (!templateId) {
      toast.error('Template id cannot be empty');
      return;
    }
    const validationError = validateDraft(draft);
    if (validationError) {
      toast.error(validationError);
      return;
    }

    setSaving(true);
    try {
      await invoke<TemplateInfo>('api_create_template', {
        templateId,
        templateJson: JSON.stringify(draftToPayload(draft)),
      });
      await loadTemplates();
      setSelectedId(templateId);
      setIsCustom(true);
      setSaveAsOpen(false);
      toast.success('Template created');
    } catch (error) {
      console.error('Failed to create template:', error);
      toast.error('Failed to create template', { description: String(error) });
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await invoke('api_delete_template', { templateId: pendingDelete });
      const list = await loadTemplates();
      // Deleting a custom override of a built-in id reverts to the built-in version.
      const reverted = list.find(t => t.id === pendingDelete);
      if (reverted && !reverted.is_custom) {
        void loadTemplate(pendingDelete);
        setIsCustom(false);
      } else {
        setSelectedId(null);
        setDraft(emptyDraft());
      }
      toast.success('Custom template deleted');
    } catch (error) {
      console.error('Failed to delete template:', error);
      toast.error('Failed to delete template', { description: String(error) });
    } finally {
      setDeleting(false);
      setPendingDelete(null);
    }
  };

  const selectedInfo = templates.find(t => t.id === selectedId);

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
      <h3 className="text-lg font-semibold text-gray-900 mb-2">Summary Templates</h3>
      <p className="text-sm text-gray-600 mb-6">
        Edit the sections and instructions used to generate meeting summaries. Custom templates are
        stored in your app data folder and override built-in templates with the same id.
      </p>

      {/* Template picker */}
      <div className="flex items-center gap-2 mb-6">
        <Select value={selectedId ?? ''} onValueChange={handleSelectTemplate}>
          <SelectTrigger className="flex-1">
            <SelectValue placeholder="Select a template to edit" />
          </SelectTrigger>
          <SelectContent>
            {templates.map(template => (
              <SelectItem key={template.id} value={template.id}>
                {template.name}
                {template.is_custom ? ' (custom)' : ''}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button variant="outline" size="sm" onClick={handleNewTemplate}>
          <Plus className="w-4 h-4" />
          New template
        </Button>
      </div>

      {loading ? (
        <div className="flex items-center gap-2 text-sm text-gray-500">
          <Loader2 className="w-4 h-4 animate-spin" />
          Loading template...
        </div>
      ) : (selectedId || saveAsOpen) && (
        <>
          {/* Name + description */}
          <div className="space-y-4 mb-6">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Name</label>
              <Input
                value={draft.name}
                onChange={e => setDraft(prev => ({ ...prev, name: e.target.value }))}
                placeholder="Template name"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Description</label>
              <Input
                value={draft.description}
                onChange={e => setDraft(prev => ({ ...prev, description: e.target.value }))}
                placeholder="What this template is for"
              />
            </div>
          </div>

          {/* Sections */}
          <div className="space-y-3 mb-6">
            <label className="block text-sm font-medium text-gray-700">Sections</label>
            {draft.sections.map((section, index) => (
              <div key={index} className="border border-gray-200 rounded-lg p-4 space-y-3">
                <div className="flex items-center gap-2">
                  <Input
                    value={section.title}
                    onChange={e => updateSection(index, { title: e.target.value })}
                    placeholder="Section title (e.g. Action Items)"
                    className="flex-1"
                  />
                  <Select
                    value={section.format}
                    onValueChange={value => updateSection(index, { format: value })}
                  >
                    <SelectTrigger className="w-32">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {SECTION_FORMATS.map(format => (
                        <SelectItem key={format} value={format}>
                          {format}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => moveSection(index, -1)}
                    disabled={index === 0}
                    className="h-8 w-8 text-gray-400 hover:text-gray-700"
                    title="Move up"
                  >
                    <ChevronUp className="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => moveSection(index, 1)}
                    disabled={index === draft.sections.length - 1}
                    className="h-8 w-8 text-gray-400 hover:text-gray-700"
                    title="Move down"
                  >
                    <ChevronDown className="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => removeSection(index)}
                    className="h-8 w-8 text-gray-400 hover:text-red-600 hover:bg-red-50"
                    title="Remove section"
                  >
                    <Trash2 className="w-4 h-4" />
                  </Button>
                </div>
                <Textarea
                  value={section.instruction}
                  onChange={e => updateSection(index, { instruction: e.target.value })}
                  placeholder="Instruction for the AI on what to extract in this section"
                  className="min-h-[60px]"
                />
                {section.format === 'list' && (
                  <Textarea
                    value={section.item_format ?? ''}
                    onChange={e => updateSection(index, { item_format: e.target.value })}
                    placeholder="Optional: item format hint, e.g. | Owner | Task | Due |"
                    className="min-h-[40px] font-mono text-xs"
                  />
                )}
              </div>
            ))}
            <Button variant="outline" size="sm" onClick={addSection}>
              <Plus className="w-4 h-4" />
              Add section
            </Button>
          </div>

          {/* Save-as-new id input */}
          {saveAsOpen && (
            <div className="flex items-center gap-2 mb-4 p-3 border border-gray-200 rounded-lg bg-gray-50">
              <label className="text-sm text-gray-600 whitespace-nowrap">Template id</label>
              <Input
                value={saveAsId}
                onChange={e => setSaveAsId(e.target.value)}
                placeholder="custom-my-template"
                className="flex-1 font-mono text-sm"
              />
              <Button size="sm" onClick={() => void handleSaveAsNew()} disabled={saving}>
                {saving && <Loader2 className="w-4 h-4 animate-spin" />}
                Create
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setSaveAsOpen(false)}>
                Cancel
              </Button>
            </div>
          )}

          {/* Actions */}
          <div className="flex items-center gap-2">
            {selectedId && (
              <Button size="sm" onClick={() => void handleSave()} disabled={saving}>
                {saving && <Loader2 className="w-4 h-4 animate-spin" />}
                Save
              </Button>
            )}
            {!saveAsOpen && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  setSaveAsId(suggestedTemplateId(draft.name));
                  setSaveAsOpen(true);
                }}
              >
                Save as new
              </Button>
            )}
            {selectedId && isCustom && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setPendingDelete(selectedId)}
                className="ml-auto text-red-600 hover:text-red-700 hover:bg-red-50"
              >
                Delete
              </Button>
            )}
          </div>

          {selectedId && !isCustom && (
            <p className="mt-3 text-xs text-gray-500">
              "{selectedInfo?.name ?? selectedId}" is a built-in template. Saving creates a custom
              copy that overrides it; you can delete the override later to revert.
            </p>
          )}
        </>
      )}

      <ConfirmationModal
        isOpen={pendingDelete !== null}
        onCancel={() => setPendingDelete(null)}
        onConfirm={() => void handleDelete()}
        text={
          pendingDelete
            ? `Delete the custom template "${selectedInfo?.name ?? pendingDelete}"?`
            : ''
        }
      />
    </div>
  );
}
