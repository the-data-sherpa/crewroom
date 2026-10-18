import { useEffect, useState } from 'react';
import { effortLevels, type Role, type Workspace } from '../shared/types';
import type { Action } from './main';
const blank: Role = { id: '', name: '', category: '', purpose: '', instructions: '', model: '', effort: 'default', enabled: true };
export function RoleEditor({ roles, action, busy }: { roles: Role[]; action: Action; busy: boolean }) {
  const [selected, setSelected] = useState('new');
  const [draft, setDraft] = useState<Role>({ ...blank });
  const [original, setOriginal] = useState<Role>({ ...blank });
  const [pending, setPending] = useState<string>();
  const [latest, setLatest] = useState<Role>();
  const unsaved = JSON.stringify(draft) !== JSON.stringify(original);
  useEffect(() => {
    const prevent = (event: BeforeUnloadEvent) => { if (unsaved) event.preventDefault(); };
    addEventListener('beforeunload', prevent); return () => removeEventListener('beforeunload', prevent);
  }, [unsaved]);
  const change = <K extends keyof Role>(field: K, value: Role[K]) => setDraft(current => ({ ...current, [field]: value }));
  const select = (id: string) => { const role = roles.find(item => item.id === id) ?? blank; setSelected(id); setDraft({ ...role }); setOriginal({ ...role }); setLatest(undefined); setPending(undefined); };
  const pushed = roles.find(role => role.id === selected);
  const current = latest && (!pushed || (latest.revision ?? 1) > (pushed.revision ?? 1)) ? latest : pushed;
  const changedElsewhere = selected !== 'new' && current && (current.revision ?? 1) !== (original.revision ?? 1);
  return <>
    <p className="muted">Reusable roles are saved in your user configuration. Changes apply to new assignments; existing assignments retain their original instructions and model.</p>
    <label>Role to edit<select value={selected} disabled={busy} onChange={event => { const id = event.target.value; if (id === selected) return; if (unsaved) setPending(id); else select(id); }}><option value="new">Create a role</option>{roles.map(role => <option value={role.id} key={role.id}>{role.name}</option>)}</select></label>
    {pending && <div className="callout" role="group" aria-label="Unsaved role changes">Unsaved changes to {draft.name || 'the new role'}.
      <div className="form-actions"><button type="button" onClick={() => setPending(undefined)}>Keep editing</button><button type="button" onClick={() => select(pending)}>Discard changes and switch</button></div>
    </div>}
    {selected !== 'new' && <p className="muted">Editing revision {original.revision ?? 1}{original.learningProposalId ? ` · Saved by learning proposal ${original.learningProposalId}` : ''}{unsaved ? ' · Unsaved changes' : ''}</p>}
    {changedElsewhere && <p className="callout" role="status">Current definition is revision {current.revision ?? 1}; your draft is based on revision {original.revision ?? 1}. Save will be refused until you reload the current definition.</p>}
    {changedElsewhere && <button type="button" disabled={busy} onClick={() => { setDraft({ ...current }); setOriginal({ ...current }); setLatest(undefined); }}>Reload current definition (discard draft)</button>}
    <form onSubmit={async event => {
      event.preventDefault();
      const saved = await action(selected === 'new' ? '/roles' : `/roles/${selected}`, selected === 'new' ? draft : { ...draft, revision: original.revision ?? 1 }, 'Role saved. Existing assignments retained.');
      if (saved) { const role = (saved as Role[]).find(item => item.id === draft.id)!; setSelected(role.id); setDraft({ ...role }); setOriginal({ ...role }); setLatest(undefined); }
      else if (selected !== 'new') {
        try { const response = await fetch('/api/workspace'); if (response.ok) setLatest(((await response.json()) as Workspace).roles.find(role => role.id === selected)); } catch { /* The draft remains available even if the current catalog cannot be fetched. */ }
      }
    }}><fieldset disabled={busy}>
      <label>Role ID<input value={draft.id} onChange={event => change('id', event.target.value)} disabled={selected !== 'new'} pattern={'[a-z][a-z0-9\\-]{0,63}'} required aria-describedby="role-id-help" /></label><p className="hint" id="role-id-help">A stable name such as performance-reviewer.</p>
      <label>Role name<input value={draft.name} onChange={event => change('name', event.target.value)} maxLength={100} required /></label>
      <label>Category<input value={draft.category} onChange={event => change('category', event.target.value)} maxLength={60} required /></label>
      <label>Purpose<textarea value={draft.purpose} onChange={event => change('purpose', event.target.value)} rows={2} maxLength={1000} required /></label>
      <label>Instructions<textarea value={draft.instructions} onChange={event => change('instructions', event.target.value)} rows={5} maxLength={12000} required /></label>
      <label>Model selector<input value={draft.model} onChange={event => change('model', event.target.value)} maxLength={200} placeholder="provider/model" aria-describedby="role-model-help" /></label><p className="hint" id="role-model-help">Use an OMP selector, or leave blank to use the configured default. Availability is checked before launch.</p>
      <label>Reasoning effort<select value={draft.effort} onChange={event => change('effort', event.target.value as Role['effort'])}><option value="default">Configured default</option>{effortLevels.map(effort => <option value={effort} key={effort}>{effort}</option>)}</select></label>
      <label className="checkbox"><input type="checkbox" checked={draft.enabled} onChange={event => change('enabled', event.target.checked)} disabled={['coordinator', 'triage', 'implementer', 'reviewer'].includes(draft.id)} />Enabled for new assignments</label>
      <button className="primary">Save role</button>
    </fieldset></form>
  </>;
}
