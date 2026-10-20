import { useEffect, useRef, useState } from 'react';
import { Mic, MicOff } from 'lucide-react';

type SpeechResult = { transcript: string };
type SpeechEvent = {
  resultIndex: number;
  results: ArrayLike<ArrayLike<SpeechResult>>;
};
type SpeechRecognitionLike = {
  lang: string;
  interimResults: boolean;
  onresult: ((event: SpeechEvent) => void) | null;
  onerror: (() => void) | null;
  onend: (() => void) | null;
  start: () => void;
  stop: () => void;
};
type SpeechWindow = Window & {
  SpeechRecognition?: new () => SpeechRecognitionLike;
  webkitSpeechRecognition?: new () => SpeechRecognitionLike;
};

/** Dictate into the chat composer using the browser's speech recognition. */
export function VoiceInputButton({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const [listening, setListening] = useState(false);
  const [unsupported, setUnsupported] = useState(false);
  const [speechError, setSpeechError] = useState<string | null>(null);
  const [showPrivacyNotice, setShowPrivacyNotice] = useState(false);
  const recognitionRef = useRef<SpeechRecognitionLike | null>(null);
  const baseValueRef = useRef(value);

  useEffect(
    () => () => {
      recognitionRef.current?.stop();
    },
    []
  );

  const startListening = () => {
    const SpeechRecognition =
      (window as SpeechWindow).SpeechRecognition ??
      (window as SpeechWindow).webkitSpeechRecognition;
    if (!SpeechRecognition) {
      setUnsupported(true);
      return;
    }

    const recognition = new SpeechRecognition();
    recognition.lang = navigator.language || 'fr-FR';
    recognition.interimResults = false;
    baseValueRef.current = value;
    recognition.onresult = (event) => {
      const transcript = Array.from(event.results)
        .slice(event.resultIndex)
        .map((result) => result[0]?.transcript ?? '')
        .join(' ')
        .trim();
      if (transcript) {
        const separator = baseValueRef.current.trim() ? '\n\n' : '';
        onChange(`${baseValueRef.current}${separator}${transcript}`);
      }
    };
    recognition.onerror = () => {
      setListening(false);
      setSpeechError('Vérifie l’autorisation du microphone et réessaie.');
    };
    recognition.onend = () => setListening(false);
    recognitionRef.current = recognition;
    setUnsupported(false);
    setSpeechError(null);
    setListening(true);
    try {
      recognition.start();
    } catch {
      setListening(false);
      setSpeechError('Impossible de démarrer le microphone. Réessaie.');
    }
  };

  return (
    <div className="relative mb-1 shrink-0">
      <button
        type="button"
        onClick={() => {
          if (listening) {
            recognitionRef.current?.stop();
            return;
          }
          if (
            localStorage.getItem('freeclaude-voice-cloud-consent') !== 'yes'
          ) {
            setShowPrivacyNotice(true);
            return;
          }
          startListening();
        }}
        disabled={unsupported}
        title={
          unsupported
            ? 'La dictée vocale n’est pas disponible dans ce navigateur'
            : listening
              ? 'Arrêter la dictée'
              : 'Dicter un message'
        }
        aria-label={
          listening ? 'Arrêter la dictée vocale' : 'Dicter un message'
        }
        className={`flex size-8 items-center justify-center rounded-full border transition-colors ${listening ? 'border-red-500 bg-red-500/10 text-red-500' : 'border-border text-muted-foreground hover:bg-accent hover:text-foreground'} disabled:cursor-not-allowed disabled:opacity-40`}
      >
        {listening ? <MicOff className="size-4" /> : <Mic className="size-4" />}
      </button>
      {showPrivacyNotice && (
        <div
          role="dialog"
          aria-label="Confidentialité de la dictée vocale"
          className="absolute bottom-10 left-0 z-50 w-72 rounded-md border border-border bg-popover p-3 text-xs text-popover-foreground shadow-lg"
        >
          <p>
            Dans la version Windows actuelle, Edge peut envoyer l’audio à
            Microsoft Azure pour le transcrire. La transcription est ensuite
            insérée dans le message.
          </p>
          <a
            href="https://learn.microsoft.com/en-us/deployedge/microsoft-edge-policies/SpeechRecognitionEnabled"
            target="_blank"
            rel="noopener noreferrer"
            className="mt-1 inline-block text-brand hover:underline"
          >
            Détails sur le traitement vocal
          </a>
          <div className="mt-3 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setShowPrivacyNotice(false)}
              className="rounded border border-border px-2 py-1 hover:bg-accent"
            >
              Annuler
            </button>
            <button
              type="button"
              onClick={() => {
                localStorage.setItem('freeclaude-voice-cloud-consent', 'yes');
                setShowPrivacyNotice(false);
                startListening();
              }}
              className="rounded bg-brand px-2 py-1 text-white hover:opacity-90"
            >
              Continuer
            </button>
          </div>
        </div>
      )}
      {speechError && (
        <span
          role="alert"
          className="absolute bottom-10 left-0 z-40 w-56 rounded border border-border bg-popover p-2 text-xs text-popover-foreground shadow"
        >
          {speechError}
        </span>
      )}
    </div>
  );
}
