import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, Loader2, Sparkles } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { BuiltInAIAPI, BuiltInModelInfo, isModelAvailable } from '@/lib/builtin-ai';
import { getSummaryModelSizeLabel } from '@/lib/onboarding-summary-model';
import { cn } from '@/lib/utils';

export function SummaryModelStep() {
  const {
    goNext,
    selectedSummaryModel,
    recommendedSummaryModel,
    setSelectedSummaryModel,
    setSummaryModelDownloaded,
  } = useOnboarding();

  const [isMac, setIsMac] = useState(false);
  const [models, setModels] = useState<BuiltInModelInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState(false);

  useEffect(() => {
    const checkPlatform = async () => {
      try {
        const { platform } = await import('@tauri-apps/plugin-os');
        setIsMac(platform() === 'macos');
      } catch (e) {
        setIsMac(navigator.userAgent.includes('Mac'));
      }
    };
    checkPlatform();
  }, []);

  useEffect(() => {
    const fetchModels = async () => {
      try {
        const list = await BuiltInAIAPI.listModels();
        setModels(list);
      } catch (error) {
        console.error('[SummaryModelStep] Failed to list summary models:', error);
        setLoadError(true);
      } finally {
        setLoading(false);
      }
    };
    fetchModels();
  }, []);

  // Fall back to the recommended model when the list cannot be loaded
  const effectiveSelection = selectedSummaryModel || recommendedSummaryModel;

  const sizeLabelFor = (model: BuiltInModelInfo) => {
    const label = getSummaryModelSizeLabel(model.name);
    if (label) return label;
    return model.size_mb > 0 ? `~${model.size_mb} MiB` : '';
  };

  return (
    <OnboardingContainer
      title="Choose a Summary Model"
      description="Summaries are generated on your device. Pick the model to download — you can change it later in settings."
      step={3}
      totalSteps={isMac ? 5 : 4}
    >
      <div className="flex flex-col items-center space-y-6">
        <div className="w-full max-w-md space-y-3">
          {loading ? (
            <div className="flex items-center justify-center py-8 text-gray-500">
              <Loader2 className="w-5 h-5 animate-spin mr-2" />
              <span className="text-sm">Loading models...</span>
            </div>
          ) : loadError ? (
            <div className="w-full bg-white rounded-lg border border-gray-200 p-4 text-sm text-gray-600">
              Could not load the model list. The recommended model will be downloaded instead.
            </div>
          ) : (
            models.map((model) => {
              const isSelected = model.name === effectiveSelection;
              const isRecommended = model.name === recommendedSummaryModel;
              const isDownloaded = isModelAvailable(model.status);
              return (
                <button
                  key={model.name}
                  type="button"
                  onClick={() => {
                    setSelectedSummaryModel(model.name);
                    setSummaryModelDownloaded(isModelAvailable(model.status));
                  }}
                  className={cn(
                    'w-full bg-white rounded-lg border p-4 text-left transition-colors flex items-start gap-3',
                    isSelected
                      ? 'border-gray-900 ring-1 ring-gray-900'
                      : 'border-gray-200 hover:border-gray-400'
                  )}
                >
                  <div className="w-10 h-10 rounded-full bg-gray-100 flex items-center justify-center flex-shrink-0">
                    <Sparkles className="w-5 h-5 text-gray-600" />
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 flex-wrap">
                      <h3 className="font-medium text-gray-900">
                        {model.display_name || model.name}
                      </h3>
                      {isRecommended && (
                        <span className="text-xs px-2 py-0.5 rounded-full bg-gray-900 text-white">
                          Recommended
                        </span>
                      )}
                      {isDownloaded && (
                        <span className="text-xs px-2 py-0.5 rounded-full bg-green-100 text-green-700">
                          Downloaded
                        </span>
                      )}
                    </div>
                    {model.description && (
                      <p className="text-sm text-gray-500 mt-1">{model.description}</p>
                    )}
                    <p className="text-sm text-gray-400 mt-1">{sizeLabelFor(model)}</p>
                  </div>
                  {isSelected && (
                    <div className="w-6 h-6 rounded-full bg-gray-900 flex items-center justify-center flex-shrink-0">
                      <Check className="w-4 h-4 text-white" />
                    </div>
                  )}
                </button>
              );
            })
          )}
        </div>

        <div className="w-full max-w-xs">
          <Button
            onClick={goNext}
            disabled={!effectiveSelection}
            className="w-full h-11 bg-gray-900 hover:bg-gray-800 text-white disabled:opacity-50 disabled:cursor-not-allowed"
          >
            Continue
          </Button>
        </div>
      </div>
    </OnboardingContainer>
  );
}
