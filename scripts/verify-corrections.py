#!/usr/bin/env python3
"""Read-only FOR-13 correction claims. Records contain evidence, never commands to run."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

REPO = Path(__file__).resolve().parents[1]
EXIT = {'verified': 0, 'incomplete': 2, 'invalid': 3}
LAYERS = {'domain_api', 'types', 'static_analysis', 'behavioral_test', 'guidance'}
MAX_JSON_BYTES = 16 * 1024 * 1024


class Problem(Exception):
    def __init__(self, outcome, reason):
        self.outcome, self.reason = outcome, reason
        super().__init__(reason)


def invalid(reason):
    raise Problem('invalid', reason)


def exact(value, required, optional=()):
    if not isinstance(value, dict) or set(value) - set(required) - set(optional) or set(required) - set(value):
        invalid(f'Expected fields {sorted(required)}, optional {sorted(optional)}; got {list(value) if isinstance(value, dict) else type(value).__name__}')


def string(value):
    if not isinstance(value, str) or not value.strip():
        invalid('Expected a nonempty string')
    return value


def digest(value):
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value):
        invalid('Expected a lowercase SHA-256')
    return value


def revision(value):
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{40}', value):
        invalid('Expected a full Git commit identity')
    return value


def strings(value):
    if not isinstance(value, list):
        invalid('Expected an array of strings')
    for item in value:
        string(item)


def timestamp(value):
    string(value)
    try:
        if datetime.fromisoformat(value.replace('Z', '+00:00')).tzinfo is None:
            invalid('Observation/review time must include a timezone, or be null for an unknown observation')
    except ValueError:
        invalid('Invalid observation/review time')


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False, allow_nan=False).encode('utf-8')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def claim_sha(record):
    return sha(canonical({key: value for key, value in record.items()
                          if key not in {'independentReview', 'validationOutputs'}}))


def strict_json(data):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                invalid(f'Duplicate JSON key: {key}')
            result[key] = value
        return result

    if len(data) > MAX_JSON_BYTES:
        invalid('JSON exceeds the 16 MiB evidence limit')
    def finite(raw):
        value = float(raw)
        if not math.isfinite(value):
            invalid('Nonfinite JSON number')
        return value
    try:
        return json.loads(data, object_pairs_hook=pairs,
                          parse_float=finite,
                          parse_constant=lambda value: invalid(f'Nonfinite JSON number: {value}'))
    except (UnicodeDecodeError, ValueError, RecursionError) as error:
        invalid(f'Malformed JSON: {error}')


def relative(value):
    string(value)
    path = PurePosixPath(value)
    if path.is_absolute() or '..' in path.parts or str(path) != value or not path.parts or '\\' in value or '\0' in value:
        invalid(f'Expected a normalized contained relative path: {value!r}')
    return value


def reference_shape(ref):
    exact(ref, {'kind', 'path', 'sha256'}, {'revision', 'excerpt', 'jsonPointer'})
    relative(ref['path']); digest(ref['sha256'])
    if ref['kind'] == 'git':
        revision(ref.get('revision'))
    elif ref['kind'] != 'private' or 'revision' in ref:
        invalid('References must be fixed Git files or private evidence-root files')
    if 'excerpt' in ref and 'jsonPointer' in ref:
        invalid('Reference extraction is ambiguous')
    if 'excerpt' in ref:
        part = ref['excerpt']
        exact(part, {'startLine', 'endLine', 'sha256'})
        if (type(part['startLine']) is not int or type(part['endLine']) is not int
                or not 1 <= part['startLine'] <= part['endLine']
                or part['endLine'] - part['startLine'] >= 200):
            invalid('Excerpt must select 1–200 inclusive lines')
        digest(part['sha256'])
    if 'jsonPointer' in ref:
        part = ref['jsonPointer']
        exact(part, {'pointer', 'sha256'})
        if not isinstance(part['pointer'], str) or (part['pointer'] and not part['pointer'].startswith('/')) or re.search(r'~(?![01])', part['pointer']):
            invalid('Invalid RFC 6901 JSON pointer')
        digest(part['sha256'])


class Resolver:
    def __init__(self, evidence_root):
        self.root = evidence_root.resolve() if evidence_root else None
        self.resolved = []
        self.problems = []

    def git(self, *args):
        result = subprocess.run(['git', '-C', str(REPO), *args], capture_output=True, timeout=30)
        if result.returncode:
            raise Problem('incomplete', 'Pinned Git evidence unavailable: ' + result.stderr.decode(errors='replace').strip()[:300])
        return result.stdout

    def resolve(self, ref, *, json_value=False):
        reference_shape(ref)
        data = None
        if ref['kind'] == 'git':
            rev = ref['revision']
            self.git('cat-file', '-e', rev + '^{commit}')
            entry = self.git('ls-tree', '-z', rev, '--', ref['path']).split(b'\0')
            expected = [item for item in entry if item and item.split(b'\t', 1)[-1].decode() == ref['path']]
            if not expected:
                raise Problem('incomplete', f'Pinned Git path unavailable: {ref["path"]}')
            if len(expected) != 1 or not expected[0].startswith((b'100644 blob ', b'100755 blob ')):
                invalid('Git reference must be a regular blob, not a symlink or directory')
            data = self.git('cat-file', 'blob', rev + ':' + ref['path'])
            actual_sha = sha(data)
        else:
            if self.root is None:
                raise Problem('incomplete', 'Private evidence requires explicit --evidence-root')
            path = (self.root / ref['path']).resolve()
            if not path.is_relative_to(self.root):
                invalid('Private evidence escapes the supplied root through a symlink')
            if not path.is_file():
                raise Problem('incomplete', f'Private evidence unavailable: {ref["path"]}')
            h = hashlib.sha256()
            with path.open('rb') as source:
                for chunk in iter(lambda: source.read(1024 * 1024), b''):
                    h.update(chunk)
            actual_sha = h.hexdigest()
            if json_value or 'excerpt' in ref or 'jsonPointer' in ref:
                if path.stat().st_size > MAX_JSON_BYTES:
                    invalid('Selected text evidence exceeds 16 MiB')
                data = path.read_bytes()
                if sha(data) != actual_sha:
                    invalid('Evidence changed while it was being read')
        if actual_sha != ref['sha256']:
            invalid(f'Source hash mismatch: {ref["path"]}')
        selected = actual_sha
        value = None
        if 'excerpt' in ref:
            part = ref['excerpt']; lines = data.splitlines(keepends=True)
            if part['endLine'] > len(lines):
                invalid('Excerpt is outside the source')
            data = b''.join(lines[part['startLine'] - 1:part['endLine']])
            selected = sha(data)
            if selected != part['sha256']:
                invalid('Excerpt hash mismatch')
        if 'jsonPointer' in ref:
            part = ref['jsonPointer']; value = strict_json(data)
            for raw in part['pointer'].split('/')[1:] if part['pointer'] else []:
                key = raw.replace('~1', '/').replace('~0', '~')
                if isinstance(value, list):
                    if not re.fullmatch(r'0|[1-9][0-9]*', key) or int(key) >= len(value):
                        invalid('JSON pointer does not select an array element')
                    value = value[int(key)]
                elif isinstance(value, dict) and key in value:
                    value = value[key]
                else:
                    invalid('JSON pointer does not resolve')
            selected = sha(canonical(value))
            if selected != part['sha256']:
                invalid('JSON pointer selection hash mismatch')
        elif json_value:
            value = strict_json(data)
        result = {'kind': ref['kind'], 'path': ref['path'], 'sourceSha256': actual_sha, 'selectionSha256': selected}
        self.resolved.append(result)
        return value, result

    def attempt(self, ref, *, json_value=False):
        try:
            return self.resolve(ref, json_value=json_value)
        except (OSError, subprocess.TimeoutExpired) as error:
            self.problems.append({'outcome': 'incomplete', 'reason': str(error)})
        except Problem as error:
            self.problems.append({'outcome': error.outcome, 'reason': error.reason})
        return None


def record_shape(record):
    exact(record, {'schemaVersion', 'id', 'issue', 'repository', 'invariant', 'scope', 'occurrences', 'remedy',
                   'enforcement', 'verification', 'disposition', 'independentReview', 'validationOutputs'})
    if type(record['schemaVersion']) is not int or record['schemaVersion'] != 1:
        invalid('Unsupported correction schemaVersion')
    for key in ('id', 'issue', 'repository', 'invariant', 'scope'):
        string(record[key])
    if record['issue'] != 'FOR-13' or record['repository'] != 'the-data-sherpa/crewroom':
        invalid('This bounded ledger requires FOR-13 and the Crewroom repository identity')
    if not isinstance(record['occurrences'], list) or not record['occurrences']:
        invalid('At least one occurrence is required')
    ids = set(); sources = set()
    for item in record['occurrences']:
        exact(item, {'id', 'observedAt', 'affectedRevision', 'behavior', 'independence', 'source'})
        for key in ('id', 'behavior', 'independence'):
            string(item[key])
        if item['affectedRevision'] is not None:
            revision(item['affectedRevision'])
        if item['observedAt'] is not None:
            timestamp(item['observedAt'])
        reference_shape(item['source'])
        if 'excerpt' not in item['source'] and 'jsonPointer' not in item['source']:
            invalid('An occurrence needs a bounded excerpt or JSON pointer')
        source = item['source']; selection = source.get('excerpt', source.get('jsonPointer'))
        source_identity = (source['sha256'], selection['sha256'])
        if item['id'] in ids or source_identity in sources:
            invalid('Duplicate incident identity or relabelled occurrence source')
        ids.add(item['id']); sources.add(source_identity)
    remedy = record['remedy']
    exact(remedy, {'primaryLayer', 'supportingLayers', 'rationale', 'strongerLayersConsidered', 'existingEnforcementInspection'})
    strings(remedy['supportingLayers'])
    if remedy['primaryLayer'] not in LAYERS or not set(remedy['supportingLayers']) <= LAYERS:
        invalid('Unsupported remedy layer')
    for key in ('rationale', 'existingEnforcementInspection'):
        string(remedy[key])
    if not isinstance(remedy['strongerLayersConsidered'], list) or not remedy['strongerLayersConsidered']:
        invalid('Explain the stronger layers considered')
    for item in remedy['strongerLayersConsidered']:
        exact(item, {'layer', 'reason'}); string(item['reason'])
        if item['layer'] not in LAYERS:
            invalid('Unsupported considered layer')
    enforcement = record['enforcement']
    exact(enforcement, {'claim', 'source', 'symbol', 'check', 'historicalBadSource', 'legitimateBehavior'})
    string(enforcement['legitimateBehavior'])
    if enforcement['claim'] not in {'mechanical', 'proposed'}:
        invalid('Enforcement claim must be mechanical or proposed')
    for key in ('source', 'check', 'historicalBadSource'):
        if enforcement[key] is not None:
            reference_shape(enforcement[key])
    if enforcement['symbol'] is not None:
        string(enforcement['symbol'])
    if enforcement['claim'] == 'mechanical' and (remedy['primaryLayer'] == 'guidance' or any(enforcement[key] is None for key in ('source', 'symbol', 'check', 'historicalBadSource'))):
        invalid('Mechanical enforcement requires code, a check and historical source; guidance cannot claim enforcement')
    verification = record['verification']
    exact(verification, {'proof', 'limits'}); strings(verification['limits'])
    if verification['proof'] is not None:
        reference_shape(verification['proof'])
    disposition = record['disposition']
    exact(disposition, {'kind', 'links'})
    if disposition['kind'] not in {'existing_enforcement_reused', 'structural_work_proposed', 'judgment_guidance_proposed'}:
        invalid('Unsupported disposition; this ledger never executes or adopts proposals')
    if (disposition['kind'] == 'existing_enforcement_reused') != (enforcement['claim'] == 'mechanical'):
        invalid('Proposed work cannot claim existing execution or enforcement')
    if not isinstance(disposition['links'], list):
        invalid('Disposition links must be an array')
    for link in disposition['links']:
        exact(link, {'kind', 'id', 'status', 'source'})
        if link['kind'] not in {'task', 'proposal', 'decision', 'revision'}:
            invalid('Unsupported disposition link')
        string(link['id']); string(link['status']); reference_shape(link['source'])
    if record['independentReview'] is not None:
        reference_shape(record['independentReview'])
    if not isinstance(record['validationOutputs'], list):
        invalid('validationOutputs must be an array')
    for ref in record['validationOutputs']:
        reference_shape(ref)


def proof_shape(proof, record, resolver):
    exact(proof, {'schemaVersion', 'kind', 'historicalBadRevision', 'currentRevision', 'historicalSourceSha256',
                  'currentSourceSha256', 'fixtureSha256', 'runs', 'cleanup', 'limits'})
    if type(proof['schemaVersion']) is not int or proof['schemaVersion'] != 1 or proof['kind'] != 'correction_enforcement':
        invalid('Unsupported enforcement proof')
    for key in ('historicalBadRevision', 'currentRevision'):
        revision(proof[key])
    for key in ('historicalSourceSha256', 'currentSourceSha256', 'fixtureSha256'):
        digest(proof[key])
    strings(proof['limits'])
    enforcement = record['enforcement']
    for role, ref in [('historical', enforcement['historicalBadSource']), ('current', enforcement['source'])]:
        expected_sha = proof['historicalSourceSha256' if role == 'historical' else 'currentSourceSha256']
        expected_revision = proof['historicalBadRevision' if role == 'historical' else 'currentRevision']
        if ref['sha256'] != expected_sha or ref.get('revision') != expected_revision:
            invalid('Proof source identity differs from the enforcement mapping')
    if proof['fixtureSha256'] != enforcement['check']['sha256']:
        invalid('Proof fixture differs from the retained check')
    if not isinstance(proof['runs'], list):
        invalid('Proof runs must be an array')
    cases = []
    for run in proof['runs']:
        exact(run, {'case', 'selected', 'executed', 'exitCode', 'outcome', 'assertion', 'sourceSha256', 'binary', 'output'})
        if run['case'] not in {'historical_bad', 'legitimate', 'payload_conflict'} or run['case'] in cases:
            invalid('Proof cases must be unique historical_bad, legitimate and payload_conflict')
        cases.append(run['case']); string(run['assertion'])
        if type(run['selected']) is not int or type(run['executed']) is not int or not 0 < run['selected'] == run['executed']:
            invalid('Proof needs an exact nonzero executed selection')
        if type(run['exitCode']) is not int:
            invalid('Proof exitCode must be an integer')
        bad = run['case'] == 'historical_bad'
        if (bad and (run['outcome'] != 'expected_failure' or run['exitCode'] <= 0)) or (not bad and (run['outcome'] != 'passed' or run['exitCode'] != 0)):
            invalid('Proof exit and behavior outcome disagree')
        if run['sourceSha256'] != proof['historicalSourceSha256' if bad else 'currentSourceSha256']:
            invalid('Run source does not match its bound proof source')
        resolver.attempt(run['binary']); resolver.attempt(run['output'])
    if set(cases) != {'historical_bad', 'legitimate', 'payload_conflict'}:
        raise Problem('incomplete', 'Missing bad-case, legitimate or changed-payload proof')
    exact(proof['cleanup'], {'status', 'receipt'})
    if proof['cleanup']['status'] != 'complete':
        raise Problem('incomplete', 'Proof cleanup is not complete')
    resolver.attempt(proof['cleanup']['receipt'])


def check_record(record, evidence_root):
    record_shape(record)
    resolver = Resolver(evidence_root)
    result = {'id': record['id'], 'claimSha256': claim_sha(record), 'occurrenceHashes': [], 'proofSha256': None,
              'recurrenceAssessment': {'distinctResolvedOccurrences': 0, 'classification': 'unresolved', 'review': 'missing'},
              'enforcementProof': {'classification': 'unproven', 'review': 'missing'},
              'disposition': record['disposition']['kind'], 'for13Gate': False}
    for occurrence in record['occurrences']:
        resolved = resolver.attempt(occurrence['source'])
        if resolved:
            _, source = resolved
            result['occurrenceHashes'].append({'id': occurrence['id'], 'sourceSha256': source['sourceSha256'],
                                               'selectionSha256': source['selectionSha256']})
    recurrence = result['recurrenceAssessment']; enforcement = result['enforcementProof']
    recurrence['distinctResolvedOccurrences'] = len(result['occurrenceHashes'])
    recurrence['classification'] = 'recurring_candidate' if len(result['occurrenceHashes']) >= 2 else 'one_off' if len(result['occurrenceHashes']) == 1 else 'unresolved'
    for key in ('source', 'check', 'historicalBadSource'):
        if record['enforcement'][key] is not None:
            resolver.attempt(record['enforcement'][key])
    for link in record['disposition']['links']:
        resolved = resolver.attempt(link['source'], json_value=True)
        if resolved:
            body, _ = resolved
            if not isinstance(body, dict) or body.get('id') != link['id'] or body.get('status') != link['status']:
                invalid('Disposition link identity/status differs from its retained source')
    for ref in record['validationOutputs']:
        resolver.attempt(ref)
    proof_ref = record['verification']['proof']
    if record['enforcement']['claim'] == 'proposed':
        enforcement['classification'] = 'not_claimed'
        if proof_ref is not None:
            invalid('Proposed guidance or structural work cannot supply a mechanical-enforcement proof')
    elif proof_ref is None:
        resolver.problems.append({'outcome': 'incomplete', 'reason': 'Mechanical enforcement proof is absent'})
    else:
        resolved = resolver.attempt(proof_ref, json_value=True)
        if resolved:
            proof, source = resolved
            result['proofSha256'] = source['selectionSha256']
            before = len(resolver.problems)
            try:
                proof_shape(proof, record, resolver)
                if before == len(resolver.problems):
                    enforcement['classification'] = 'resolved'
            except Problem as error:
                resolver.problems.append({'outcome': error.outcome, 'reason': error.reason})
    review_ref = record['independentReview']
    review = resolver.attempt(review_ref, json_value=True) if review_ref else None
    if review:
        body, _ = review
        exact(body, {'schemaVersion', 'kind', 'classId', 'claimSha256', 'reviewer', 'reviewedAt', 'occurrenceHashes',
                     'proofSha256', 'recurrence', 'enforcement', 'rationale', 'limits'})
        exact(body['reviewer'], {'identity', 'independent'}); string(body['reviewer']['identity'])
        if type(body['schemaVersion']) is not int or body['schemaVersion'] != 1 or body['kind'] != 'correction_claim_review' or body['classId'] != record['id'] or body['reviewer']['independent'] is not True:
            invalid('Independent review has an unsupported identity, kind or independence declaration')
        timestamp(body['reviewedAt']); string(body['rationale']); strings(body['limits']); digest(body['claimSha256'])
        if body['recurrence'] not in {'confirmed', 'one_off', 'unresolved'} or body['enforcement'] not in {'confirmed', 'not_claimed', 'unresolved'}:
            invalid('Unsupported review conclusion')
        if not isinstance(body['occurrenceHashes'], list):
            invalid('Review occurrenceHashes must be an array')
        for item in body['occurrenceHashes']:
            exact(item, {'id', 'sourceSha256', 'selectionSha256'}); string(item['id']); digest(item['sourceSha256']); digest(item['selectionSha256'])
        if body['proofSha256'] is not None:
            digest(body['proofSha256'])
        if body['claimSha256'] != result['claimSha256'] or body['occurrenceHashes'] != result['occurrenceHashes'] or body['proofSha256'] != result['proofSha256']:
            recurrence['review'] = enforcement['review'] = 'stale'
        elif body['recurrence'] == 'unresolved' or body['enforcement'] == 'unresolved':
            recurrence['review'] = enforcement['review'] = 'unresolved'
        else:
            expected = 'confirmed' if len(result['occurrenceHashes']) >= 2 else 'one_off'
            if body['recurrence'] != expected or body['enforcement'] != ('not_claimed' if enforcement['classification'] == 'not_claimed' else 'confirmed'):
                invalid('Review conclusion does not match the bounded claim')
            recurrence['review'] = enforcement['review'] = 'bound'
            if expected == 'confirmed':
                recurrence['classification'] = 'recurring'
    if recurrence['review'] != 'bound':
        resolver.problems.append({'outcome': 'incomplete', 'reason': 'Independent semantic review is missing, stale or unresolved'})
    result['resolvedReferences'] = resolver.resolved
    result['problems'] = resolver.problems
    result['outcome'] = 'invalid' if any(p['outcome'] == 'invalid' for p in resolver.problems) else 'incomplete' if resolver.problems else 'verified'
    result['for13Gate'] = (result['outcome'] == 'verified' and recurrence['classification'] == 'recurring'
                           and enforcement['classification'] == 'resolved' and enforcement['review'] == 'bound')
    return result


def check(directory, evidence_root):
    files = sorted(directory.glob('*.json')) if directory.is_dir() else [directory]
    if not files:
        raise Problem('incomplete', 'No correction records found')
    results = []; ids = set()
    for path in files:
        try:
            record = strict_json(path.read_bytes())
            record_shape(record)
            if record['id'] in ids:
                invalid(f'Duplicate correction class ID: {record["id"]}')
            ids.add(record['id'])
            result = check_record(record, evidence_root)
        except Problem as error:
            result = {'outcome': error.outcome, 'problems': [{'outcome': error.outcome, 'reason': error.reason}], 'for13Gate': False}
        except OSError as error:
            result = {'outcome': 'incomplete', 'problems': [{'outcome': 'incomplete', 'reason': str(error)}], 'for13Gate': False}
        except (TypeError, ValueError, KeyError, RecursionError) as error:
            result = {'outcome': 'invalid', 'problems': [{'outcome': 'invalid', 'reason': f'Malformed claim: {error}'}], 'for13Gate': False}
        result['record'] = str(path)
        results.append(result)
    outcome = 'invalid' if any(r['outcome'] == 'invalid' for r in results) else 'incomplete' if any(r['outcome'] == 'incomplete' for r in results) else 'verified'
    return {'schemaVersion': 1, 'kind': 'correction_validation', 'outcome': outcome, 'records': results,
            'for13Gate': outcome == 'verified' and any(r['for13Gate'] for r in results),
            'limits': ['Reference/hash and review-binding validation; independent semantic judgment is retained evidence, not authenticated by this CLI.',
                       'No command from a record is executed; no proposal, configuration, workflow or assignment is changed.']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    command = sub.add_parser('check')
    command.add_argument('records', type=Path)
    command.add_argument('--evidence-root', type=Path)
    command.add_argument('--output', type=Path, help='New validation receipt; existing files are refused')
    args = parser.parse_args()
    try:
        result = check(args.records, args.evidence_root)
    except Problem as error:
        result = {'schemaVersion': 1, 'outcome': error.outcome, 'problems': [error.reason], 'for13Gate': False}
    result['validatedAt'] = datetime.now(timezone.utc).isoformat()
    if args.output:
        try:
            fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, 'w') as output:
                json.dump(result, output, indent=2, ensure_ascii=False, allow_nan=False)
                output.write('\n')
        except OSError as error:
            result = {'schemaVersion': 1, 'outcome': 'invalid', 'problems': [f'New output refused: {error}'], 'for13Gate': False}
    print(json.dumps(result, indent=2, ensure_ascii=False, allow_nan=False))
    return EXIT[result['outcome']]


if __name__ == '__main__':
    sys.exit(main())
