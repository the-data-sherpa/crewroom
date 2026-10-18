import { useEffect, useRef, useState } from 'react';
import { init, Terminal } from 'ghostty-web';
import { live, decodeBase64 } from './live';

let ready: Promise<void> | undefined;
const theme = { background: '#0b0f11', foreground: '#d9e2dd', cursor: '#0b0f11', black: '#1a2226', red: '#ee7b70', green: '#7fd6b9', yellow: '#e3aa4a',
  blue: '#8fc1ee', magenta: '#c7b3f0', cyan: '#7fd0d6', white: '#d9e2dd', brightBlack: '#8c9a94', brightWhite: '#ffffff' };
const stripAnsi = (text: string) => text.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '').replace(/\x1b\][^\x07]*\x07/g, '');

type Status = { state: 'connecting' | 'live' | 'retained' | 'lagging' | 'unavailable'; reason?: string };

/**
 * A read-only view of one agent pane (or a running check's output). Keystrokes are
 * never sent anywhere: operators steer through the coordinator.
 */
export function LiveTerminal({ topic, label, convertEol = false }: { topic: string; label: string; convertEol?: boolean }) {
  const frame = useRef<HTMLDivElement>(null);
  const host = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState<Status>({ state: 'connecting' });
  const [fallback, setFallback] = useState<string>();
  const [received, setReceived] = useState(0);
  useEffect(() => {
    let disposed = false; let term: Terminal | undefined; let text = '';
    let unsubscribe: (() => void) | undefined; let observer: ResizeObserver | undefined;
    const fit = () => {
      const canvas = host.current?.querySelector('canvas'); const outer = frame.current;
      if (!canvas || !outer || !host.current) return;
      // Zoom (unlike transform) changes layout size. Fit narrow tiles, but never below legibility;
      // anything narrower scrolls sideways inside the tile rather than widening the page.
      host.current.style.zoom = '1';
      const natural = host.current.getBoundingClientRect().width;
      host.current.style.zoom = String(Math.max(0.75, Math.min(1, outer.clientWidth / natural)));
    };
    const decoder = new TextDecoder();
    const write = (bytes: Uint8Array) => {
      setReceived(count => count + bytes.length);
      // Plain command logs use bare newlines; a terminal needs CR LF or lines staircase.
      if (term && convertEol) term.write(decoder.decode(bytes, { stream: true }).replace(/\r?\n/g, '\r\n'));
      else if (term) term.write(bytes);
      else { text = (text + stripAnsi(new TextDecoder().decode(bytes))).slice(-20000); setFallback(text); }
    };
    (ready ??= init()).then(() => {
      if (disposed || !host.current) return;
      term = new Terminal({ cols: 80, rows: 24, disableStdin: true, cursorBlink: false, scrollback: 5000, fontSize: 13,
        fontFamily: '"JetBrains Mono", ui-monospace, Menlo, monospace', theme });
      term.open(host.current);
      // Read-only: keep the emulator's input element out of the tab order and accessibility tree.
      if (term.textarea) { term.textarea.tabIndex = -1; term.textarea.readOnly = true; term.textarea.setAttribute('aria-hidden', 'true'); }
      observer = new ResizeObserver(fit); if (frame.current) observer.observe(frame.current);
      requestAnimationFrame(fit);
    }).catch(() => { setFallback(''); }).finally(() => {
      if (disposed) return;
      unsubscribe = live.subscribe(topic, message => {
        if (message.kind === 'reset') {
          if (term) { term.reset(); if (term.cols !== message.cols || term.rows !== message.rows) term.resize(message.cols, message.rows); requestAnimationFrame(fit); }
          text = '';
          write(decodeBase64(message.data));
          setStatus({ state: message.live ? 'live' : 'retained' });
        } else if (message.kind === 'data') write(decodeBase64(message.data));
        else if (message.kind === 'lagging') setStatus({ state: 'lagging' });
        else if (message.kind === 'unavailable' || message.kind === 'error') setStatus({ state: 'unavailable', reason: message.reason ?? message.error });
      }, { restart: true });
    });
    return () => { disposed = true; unsubscribe?.(); observer?.disconnect(); term?.dispose(); };
  }, [topic, convertEol]);
  return <div className="terminal-frame" ref={frame} role="img" aria-label={`${label} terminal output (read-only)`} data-state={status.state} data-received={received}>
    {fallback !== undefined ? <pre className="terminal-fallback">{fallback || 'Waiting for output…'}</pre> : <div className="terminal-host" ref={host} />}
    {status.state !== 'live' && <span className={`terminal-status ${status.state}`}>{{ connecting: 'Connecting…', retained: 'Retained output · not live', lagging: 'Catching up…', unavailable: status.reason ?? 'Terminal unavailable', live: '' }[status.state]}</span>}
  </div>;
}
