import type { ReactNode } from 'react';

/**
 * Renders the subset of Markdown agents write (paragraphs, lists, tables, code, emphasis,
 * links) as React elements. Never uses raw HTML, so agent text cannot inject markup.
 */
function inline(text: string, key = 'i'): ReactNode[] {
  const out: ReactNode[] = []; let rest = text; let index = 0;
  const pattern = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\[[^\]]+\]\((https?:\/\/[^)\s]+)\))|(\*[^*\s][^*]*\*)/;
  while (rest) {
    const match = pattern.exec(rest);
    if (!match) { out.push(rest); break; }
    if (match.index) out.push(rest.slice(0, match.index));
    const token = match[0]; const id = `${key}-${index++}`;
    if (match[1]) out.push(<code key={id}>{token.slice(1, -1)}</code>);
    else if (match[2]) out.push(<strong key={id}>{inline(token.slice(2, -2), id)}</strong>);
    else if (match[3]) out.push(<a key={id} href={match[4]} target="_blank" rel="noreferrer">{token.slice(1, token.indexOf(']('))}</a>);
    else out.push(<em key={id}>{inline(token.slice(1, -1), id)}</em>);
    rest = rest.slice(match.index + token.length);
  }
  return out;
}

const cells = (line: string) => line.trim().replace(/^\||\|$/g, '').split('|').map(cell => cell.trim());

export function Markdown({ text }: { text: string }) {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  const blocks: ReactNode[] = [];
  for (let index = 0; index < lines.length;) {
    const line = lines[index]; const key = `b${index}`;
    if (!line.trim()) { index++; continue; }
    if (line.startsWith('```')) {
      const body: string[] = []; index++;
      while (index < lines.length && !lines[index].startsWith('```')) body.push(lines[index++]);
      index++; blocks.push(<pre key={key} className="md-code"><code>{body.join('\n')}</code></pre>); continue;
    }
    const heading = /^(#{1,4})\s+(.*)$/.exec(line);
    if (heading) { blocks.push(<p key={key} className="md-heading"><strong>{inline(heading[2], key)}</strong></p>); index++; continue; }
    if (line.trim().startsWith('|') && /^\s*\|?\s*:?-{2,}/.test(lines[index + 1] ?? '')) {
      const head = cells(line); const rows: string[][] = []; index += 2;
      while (index < lines.length && lines[index].trim().startsWith('|')) rows.push(cells(lines[index++]));
      blocks.push(<div key={key} className="table-scroll"><table><thead><tr>{head.map((cell, i) => <th key={i}>{inline(cell, `${key}h${i}`)}</th>)}</tr></thead>
        <tbody>{rows.map((row, r) => <tr key={r}>{row.map((cell, i) => <td key={i}>{inline(cell, `${key}${r}-${i}`)}</td>)}</tr>)}</tbody></table></div>);
      continue;
    }
    const bullet = /^(\s*)([-*]|\d+\.)\s+(.*)$/;
    if (bullet.test(line)) {
      const ordered = /\d+\./.test(bullet.exec(line)![2]); const items: string[] = [];
      while (index < lines.length && bullet.test(lines[index])) items.push(bullet.exec(lines[index++])![3]);
      const List = ordered ? 'ol' : 'ul';
      blocks.push(<List key={key}>{items.map((item, i) => <li key={i}>{inline(item, `${key}${i}`)}</li>)}</List>);
      continue;
    }
    const paragraph: string[] = [];
    while (index < lines.length && lines[index].trim() && !lines[index].startsWith('```') && !bullet.test(lines[index]) && !/^#{1,4}\s/.test(lines[index])) paragraph.push(lines[index++]);
    blocks.push(<p key={key}>{inline(paragraph.join(' '), key)}</p>);
  }
  return <div className="markdown">{blocks}</div>;
}
