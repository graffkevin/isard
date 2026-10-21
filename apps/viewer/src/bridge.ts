/**
 * The bridge's transports: messages from a host by Server-Sent Events (`?bridge=1`, the local server), `postMessage`
 * (an iframe's parent, a webview) or `window.isardViewer.send`; messages to the host by POST, `postMessage` and the
 * listeners of `window.isardViewer.on`.
 */
import { type HostMessage, isHostMessage, isRecord, type PostedMessage, type ViewerMessage } from './protocol.ts';

type Listener = (message: ViewerMessage) => void;

/** What the page exposes as `window.isardViewer` */
export interface ViewerApi {
  /** Protocol version */
  protocol: number;
  /** A host message, applied by the viewer */
  send(message: HostMessage): void;
  /** Listens to the viewer's messages of a type (`'*'` for all); returns the function that stops listening */
  on(type: ViewerMessage['type'] | '*', listener: Listener): () => void;
}

declare global {
  interface Window {
    isardViewer?: ViewerApi;
  }
}

export class Bridge {
  private readonly listeners = new Map<string, Set<Listener>>();
  /** Viewer messages sent before the page's own listeners or the host are there are not kept: hosts ask for the
   * state on `ready` */
  private readonly http: boolean;
  private events: EventSource | undefined;

  constructor(
    private readonly apply: (message: HostMessage) => void,
    options: { http: boolean },
  ) {
    this.http = options.http;
    window.isardViewer = {
      protocol: 1,
      send: (message) => this.receive(message),
      on: (type, listener) => {
        const set = this.listeners.get(type) ?? new Set<Listener>();

        this.listeners.set(type, set);
        set.add(listener);

        return () => set.delete(listener);
      },
    };
    // An iframe's parent or the window that opened this one; other windows are not listened to
    window.addEventListener('message', (event: MessageEvent<unknown>) => {
      if (event.source !== window.parent && event.source !== window.opener) return;

      const data = event.data;

      if (!isRecord(data) || data.isard !== 1) return;

      const { isard: _, ...message } = data;

      this.receive(message);
    });
    if (this.http) this.connect();
  }

  /** The host's events (the server replays its state on each connection: a reconnection gets it again) */
  private connect(): void {
    this.events = new EventSource('bridge/events');
    this.events.onmessage = (event: MessageEvent<string>) => {
      try {
        const message: unknown = JSON.parse(event.data);

        (Array.isArray(message) ? message : [message]).forEach((m: unknown) => {
          this.receive(m);
        });
      } catch (error) {
        this.send({ type: 'error', message: `unreadable bridge event: ${String(error)}` });
      }
    };
  }

  /** A host message from any transport, anything else ignored */
  private receive(message: unknown): void {
    if (isHostMessage(message)) this.apply(message);
  }

  /** A message to the host, by every transport there is */
  send(message: ViewerMessage): void {
    if (this.http) {
      fetch('bridge/message', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(message),
        keepalive: true,
      }).catch(() => undefined);
    }
    const opener: Window | null = window.opener;

    if (window.parent !== window) window.parent.postMessage({ isard: 1, ...message } satisfies PostedMessage, '*');
    else opener?.postMessage({ isard: 1, ...message } satisfies PostedMessage, '*');
    [message.type, '*'].forEach((type) => {
      this.listeners.get(type)?.forEach((listener) => {
        try {
          listener(message);
        } catch (error) {
          console.error('isard viewer: a listener failed', error);
        }
      });
    });
  }
}
