'use client';

import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { configService, CustomOpenAIConfig } from '@/services/configService';
import { useConfig } from '@/contexts/ConfigContext';
import { Loader2, CheckCircle2, ChevronDown, ChevronUp, Server } from 'lucide-react';

/**
 * Standalone "Custom endpoint" settings card for the summary model settings
 * page. Edits the OpenAI-compatible endpoint config persisted via
 * `api_save_custom_openai_config` and can activate `custom-openai` as the
 * summary provider.
 *
 * Mount inside `SummaryModelSettings.tsx` (component owned elsewhere) in the
 * top-level `flex flex-col gap-4` stack, e.g. after `<SummaryLanguageSettings />`:
 *
 *   <CustomEndpointSettings />
 */
export function CustomEndpointSettings() {
  const { modelConfig } = useConfig();
  const isActiveProvider = modelConfig?.provider === 'custom-openai';

  const [endpoint, setEndpoint] = useState('');
  const [model, setModel] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [maxTokens, setMaxTokens] = useState('');
  const [temperature, setTemperature] = useState('');
  const [topP, setTopP] = useState('');
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [activating, setActivating] = useState(false);

  useEffect(() => {
    const load = async () => {
      try {
        const config = await configService.getCustomOpenAIConfig();
        if (config) {
          setEndpoint(config.endpoint ?? '');
          setModel(config.model ?? '');
          setApiKey(config.apiKey ?? '');
          setMaxTokens(config.maxTokens != null ? String(config.maxTokens) : '');
          setTemperature(config.temperature != null ? String(config.temperature) : '');
          setTopP(config.topP != null ? String(config.topP) : '');
        }
      } catch (error) {
        console.error('Failed to load custom endpoint config:', error);
      } finally {
        setLoading(false);
      }
    };
    load();
  }, []);

  const buildConfig = (): CustomOpenAIConfig => ({
    endpoint: endpoint.trim(),
    apiKey: apiKey.trim() || null,
    model: model.trim(),
    maxTokens: maxTokens.trim() ? parseInt(maxTokens, 10) : null,
    temperature: temperature.trim() ? parseFloat(temperature) : null,
    topP: topP.trim() ? parseFloat(topP) : null,
  });

  const saveConfig = async (): Promise<boolean> => {
    try {
      await configService.saveCustomOpenAIConfig(buildConfig());
      return true;
    } catch (error) {
      console.error('Failed to save custom endpoint config:', error);
      toast.error('Failed to save custom endpoint', {
        description: error instanceof Error ? error.message : String(error),
      });
      return false;
    }
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      if (await saveConfig()) {
        toast.success('Custom endpoint saved');
      }
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    if (!endpoint.trim() || !model.trim()) {
      toast.error('Please enter endpoint URL and model name first');
      return;
    }
    setTesting(true);
    try {
      const result = await configService.testCustomOpenAIConnection(
        endpoint.trim(),
        apiKey.trim() || null,
        model.trim()
      );
      toast.success(result.message || 'Connection successful!');
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setTesting(false);
    }
  };

  const handleActivate = async () => {
    setActivating(true);
    try {
      if (!(await saveConfig())) return;
      const config = buildConfig();
      const current = (await invoke('api_get_model_config')) as any;
      const updatedConfig = {
        provider: 'custom-openai',
        model: config.model,
        whisperModel: current?.whisperModel ?? 'large-v3',
        apiKey: null,
        ollamaEndpoint: null,
        customOpenAIEndpoint: config.endpoint,
        customOpenAIModel: config.model,
        customOpenAIApiKey: config.apiKey,
        maxTokens: config.maxTokens,
        temperature: config.temperature,
        topP: config.topP,
      };
      await invoke('api_save_model_config', {
        provider: 'custom-openai',
        model: config.model,
        whisperModel: updatedConfig.whisperModel,
        apiKey: null,
        ollamaEndpoint: null,
      });
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', updatedConfig);
      toast.success('Custom endpoint is now the summary provider');
    } catch (error) {
      console.error('Failed to activate custom endpoint:', error);
      toast.error('Failed to activate custom endpoint');
    } finally {
      setActivating(false);
    }
  };

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
      <div className="flex items-center gap-2 mb-2">
        <Server size={18} className="text-gray-500" />
        <h3 className="text-lg font-semibold text-gray-900">Custom Endpoint</h3>
      </div>
      <p className="text-sm text-gray-600 mb-6">
        Connect to any OpenAI-compatible API server (LM Studio, vLLM, llama.cpp
        server, a self-hosted gateway, ...). Used when the summary provider is
        set to &quot;Custom Server (OpenAI)&quot;.
      </p>

      {loading ? (
        <div className="flex items-center gap-2 text-sm text-gray-500">
          <Loader2 className="w-4 h-4 animate-spin" />
          Loading endpoint settings...
        </div>
      ) : (
        <div className="space-y-4">
          <div>
            <Label htmlFor="custom-endpoint-url">Endpoint URL *</Label>
            <Input
              id="custom-endpoint-url"
              value={endpoint}
              onChange={(e) => setEndpoint(e.target.value)}
              placeholder="http://localhost:8000/v1"
              className="mt-1"
            />
            <p className="text-xs text-gray-500 mt-1">
              Base URL of the OpenAI-compatible API
            </p>
          </div>

          <div>
            <Label htmlFor="custom-endpoint-model">Model Name *</Label>
            <Input
              id="custom-endpoint-model"
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder="gpt-4, llama-3-70b, etc."
              className="mt-1"
            />
          </div>

          <div>
            <Label htmlFor="custom-endpoint-api-key">API Key (optional)</Label>
            <Input
              id="custom-endpoint-api-key"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="Leave empty if not required"
              className="mt-1"
            />
          </div>

          <div>
            <div
              className="flex items-center justify-between cursor-pointer py-2"
              onClick={() => setAdvancedOpen(!advancedOpen)}
            >
              <Label className="cursor-pointer">Advanced Options</Label>
              {advancedOpen ? (
                <ChevronUp className="h-4 w-4 text-gray-400" />
              ) : (
                <ChevronDown className="h-4 w-4 text-gray-400" />
              )}
            </div>

            {advancedOpen && (
              <div className="space-y-3 pl-2 border-l-2 border-gray-200 mt-2">
                <div>
                  <Label htmlFor="custom-endpoint-max-tokens">Max Tokens</Label>
                  <Input
                    id="custom-endpoint-max-tokens"
                    type="number"
                    value={maxTokens}
                    onChange={(e) => setMaxTokens(e.target.value)}
                    placeholder="e.g., 4096"
                    className="mt-1"
                  />
                </div>
                <div>
                  <Label htmlFor="custom-endpoint-temperature">Temperature (0.0-2.0)</Label>
                  <Input
                    id="custom-endpoint-temperature"
                    type="number"
                    step="0.1"
                    min="0"
                    max="2"
                    value={temperature}
                    onChange={(e) => setTemperature(e.target.value)}
                    placeholder="e.g., 0.7"
                    className="mt-1"
                  />
                </div>
                <div>
                  <Label htmlFor="custom-endpoint-top-p">Top P (0.0-1.0)</Label>
                  <Input
                    id="custom-endpoint-top-p"
                    type="number"
                    step="0.1"
                    min="0"
                    max="1"
                    value={topP}
                    onChange={(e) => setTopP(e.target.value)}
                    placeholder="e.g., 0.9"
                    className="mt-1"
                  />
                </div>
              </div>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-2 pt-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleTest}
              disabled={testing || !endpoint.trim() || !model.trim()}
            >
              {testing ? (
                <>
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  Testing...
                </>
              ) : (
                <>
                  <CheckCircle2 className="mr-2 h-4 w-4" />
                  Test Connection
                </>
              )}
            </Button>

            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleSave}
              disabled={saving || !endpoint.trim() || !model.trim()}
            >
              {saving && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              Save
            </Button>

            {!isActiveProvider && (
              <Button
                type="button"
                size="sm"
                onClick={handleActivate}
                disabled={activating || !endpoint.trim() || !model.trim()}
              >
                {activating && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                Use as summary provider
              </Button>
            )}
            {isActiveProvider && (
              <span className="text-xs text-green-700 bg-green-50 border border-green-200 rounded-full px-3 py-1">
                Active summary provider
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
