import { useEffect, useState, type ReactNode } from "react";
import { Volume2, VolumeX } from "lucide-react";

export interface ChatAssistantMessageRenderProps {
  content: string;
  workspaceId?: string;
}

interface ChatAssistantMessageProps {
  content: string;
  workspaceId?: string;
  renderMarkdown: (props: ChatAssistantMessageRenderProps) => ReactNode;
}

export function ChatAssistantMessage({
  content,
  workspaceId,
  renderMarkdown,
}: ChatAssistantMessageProps) {
  const [speaking, setSpeaking] = useState(false);
  useEffect(
    () => () => {
      if (speaking) window.speechSynthesis?.cancel();
    },
    [speaking],
  );

  const toggleSpeech = () => {
    if (!window.speechSynthesis) return;
    if (speaking) {
      window.speechSynthesis.cancel();
      setSpeaking(false);
      return;
    }

    const utterance = new SpeechSynthesisUtterance(content);
    utterance.lang = navigator.language || "fr-FR";
    utterance.onend = () => setSpeaking(false);
    utterance.onerror = () => setSpeaking(false);
    setSpeaking(true);
    window.speechSynthesis.cancel();
    window.speechSynthesis.speak(utterance);
  };

  return (
    <div className="group/assistant-message relative">
      {renderMarkdown({ content, workspaceId })}
      <button
        type="button"
        onClick={toggleSpeech}
        aria-label={
          speaking
            ? "Arrêter la lecture vocale"
            : "Lire la réponse à voix haute"
        }
        title={speaking ? "Arrêter la lecture" : "Lire à voix haute"}
        className="absolute right-1 top-1 flex size-7 items-center justify-center rounded-md bg-background/80 text-muted-foreground opacity-0 shadow-sm transition-opacity hover:text-foreground focus:opacity-100 group-hover/assistant-message:opacity-100"
      >
        {speaking ? (
          <VolumeX className="size-4" />
        ) : (
          <Volume2 className="size-4" />
        )}
      </button>
    </div>
  );
}
