"use client";

import { useState, useCallback, useRef, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Loader2, Send, MessageSquare } from 'lucide-react';
import Analytics from '@/lib/analytics';

interface ChatTurn {
  question: string;
  answer: string;
}

interface MeetingChatProps {
  meetingId: string;
}

/**
 * "Ask your meeting" chat panel — sends each question to the `ask_meeting`
 * Tauri command, which loads transcript + summary context server-side and
 * answers with the configured summary provider. Conversation state is kept
 * client-side and prior turns are sent back for multi-turn context.
 */
export function MeetingChat({ meetingId }: MeetingChatProps) {
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [question, setQuestion] = useState('');
  const [asking, setAsking] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Reset conversation when switching meetings
  useEffect(() => {
    setTurns([]);
    setQuestion('');
    setAsking(false);
  }, [meetingId]);

  // Scroll to the latest exchange
  useEffect(() => {
    const viewport = scrollRef.current?.querySelector<HTMLElement>(
      '[data-radix-scroll-area-viewport]'
    );
    if (viewport) {
      viewport.scrollTop = viewport.scrollHeight;
    }
  }, [turns, asking]);

  const handleAsk = useCallback(async () => {
    const q = question.trim();
    if (!q || asking || !meetingId) return;
    Analytics.trackButtonClick('ask_meeting', 'meeting_details');
    setAsking(true);
    setQuestion('');
    try {
      const answer = await invoke<string>('ask_meeting', {
        meetingId,
        question: q,
        history: turns,
      });
      setTurns((prev) => [...prev, { question: q, answer }]);
    } catch (err) {
      toast.error('Could not get an answer', {
        description: err instanceof Error ? err.message : String(err),
      });
      setQuestion(q);
    } finally {
      setAsking(false);
    }
  }, [question, asking, meetingId, turns]);

  return (
    <div className="flex-1 min-h-0 min-w-0 flex flex-col bg-white overflow-hidden">
      <div className="p-4 border-b border-gray-200">
        <div className="flex items-center gap-2">
          <MessageSquare size={18} className="text-gray-500" />
          <h3 className="text-sm font-semibold text-gray-900">Ask your meeting</h3>
        </div>
      </div>

      <ScrollArea ref={scrollRef} className="flex-1 min-h-0">
        <div className="p-4 space-y-4">
          {turns.length === 0 && !asking && (
            <p className="text-sm text-gray-500">
              Ask a question about this meeting&apos;s transcript, e.g.
              &quot;What action items were assigned?&quot;
            </p>
          )}
          {turns.map((turn, index) => (
            <div key={index} className="space-y-2">
              <div className="flex justify-end">
                <div dir="auto" className="max-w-[85%] rounded-lg bg-blue-600 px-3 py-2 text-sm text-white whitespace-pre-wrap">
                  {turn.question}
                </div>
              </div>
              <div className="flex justify-start">
                <div dir="auto" className="max-w-[85%] rounded-lg bg-gray-100 px-3 py-2 text-sm text-gray-800 whitespace-pre-wrap">
                  {turn.answer}
                </div>
              </div>
            </div>
          ))}
          {asking && (
            <div className="flex justify-start">
              <div className="flex items-center gap-2 rounded-lg bg-gray-100 px-3 py-2 text-sm text-gray-500">
                <Loader2 size={14} className="animate-spin" />
                Thinking...
              </div>
            </div>
          )}
        </div>
      </ScrollArea>

      <div className="p-4 border-t border-gray-200">
        <form
          className="flex items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            handleAsk();
          }}
        >
          <Input
            dir="auto"
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            placeholder="Ask about this meeting..."
            disabled={asking}
          />
          <Button
            type="submit"
            size="icon"
            variant="outline"
            disabled={asking || !question.trim()}
            title="Ask"
          >
            {asking ? <Loader2 className="animate-spin" /> : <Send />}
          </Button>
        </form>
      </div>
    </div>
  );
}
