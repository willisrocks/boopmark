import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixtureSessionAllowed, buttonAllowed, projectAccessibility } from '../../../scripts/e2e/chrome-popup-control.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const local = {
  CHROME_EXTENSION_SESSION: 'boopmark-extension-local-fdcc',
  CHROME_EXTENSION_PROFILE: path.join(root, '.cache/boopmark-extension/local-qa-profile'),
};

test('fixture controls require both exact local session and isolated profile', () => {
  assert.equal(fixtureSessionAllowed(local), true);
  for (const env of [
    {}, { ...local, CHROME_EXTENSION_SESSION: 'boopmark-extension-production-fdcc' },
    { ...local, CHROME_EXTENSION_PROFILE: '/workspace/.cache/boopmark-extension/production-profile' },
    { ...local, CHROME_EXTENSION_PROFILE: '.cache/boopmark-extension/local-qa-profile' },
    { ...local, CHROME_EXTENSION_PROFILE: `${local.CHROME_EXTENSION_PROFILE}/../production-profile` },
    { ...local, CHROME_EXTENSION_PROFILE: `/different-root${local.CHROME_EXTENSION_PROFILE}` },
  ]) {
    assert.equal(fixtureSessionAllowed(env), false);
    for (const button of ['settings-button', 'back-button', 'disconnect-button']) assert.equal(buttonAllowed(button, env), false);
  }
});

test('local settings controls do not widen capture controls or expose key entry', () => {
  for (const button of ['settings-button', 'back-button', 'disconnect-button']) assert.equal(buttonAllowed(button, local), true);
  for (const button of ['autofill-button', 'save-button', 'cancel-button', 'close-button', 'ack-button']) assert.equal(buttonAllowed(button, {}), true);
  for (const env of [{}, local]) {
    assert.equal(buttonAllowed('connect-button', env), false);
    assert.equal(buttonAllowed('api-key', env), false);
  }
});

test('capture AX projection preserves labels and live properties but never field values', () => {
  const nodes = [
    { backendDOMNodeId: 1, role: { value: 'textbox' }, name: { value: 'Title optional' }, value: { value: 'private field value' }, properties: [{ name: 'focused', value: { value: true } }, { name: 'valuetext', value: { value: 'private field value' } }] },
    { backendDOMNodeId: 2, role: { value: 'status' }, name: { value: '' }, properties: [{ name: 'live', value: { value: 'polite' } }, { name: 'relevant', value: { value: 'additions text' } }, { name: 'atomic', value: { value: true } }] },
    { backendDOMNodeId: 3, role: { value: 'StaticText' }, name: { value: 'Fetching metadata…' } },
  ];
  const projected = projectAccessibility(nodes, [
    { id: 'title', kind: 'element', backendDOMNodeId: 1 },
    { id: 'metadata-status', kind: 'element', backendDOMNodeId: 2 },
    { id: 'metadata-status', kind: 'status-text', backendDOMNodeId: 3 },
  ]);
  assert.deepEqual(projected, [
    { id: 'title', kind: 'element', role: 'textbox', name: 'Title optional', focused: true },
    { id: 'metadata-status', kind: 'element', role: 'status', name: '', live: 'polite', relevant: 'additions text', atomic: true },
    { id: 'metadata-status', kind: 'status-text', role: 'StaticText', name: 'Fetching metadata…' },
  ]);
  assert.equal(JSON.stringify(projected).includes('private field value'), false);
});

test('capture AX projection drops setup, credential, unrelated and ignored nodes', () => {
  const observed = ['api-key', 'server', 'setup-heading', 'connection-status', 'title'].map((id, index) => ({ id, kind: 'element', backendDOMNodeId: index + 1 }));
  const nodes = observed.map(item => ({ backendDOMNodeId: item.backendDOMNodeId, ignored: item.id === 'title', role: { value: 'textbox' }, name: { value: 'must not leak' }, value: { value: 'secret' } }));
  nodes.push({ backendDOMNodeId: 999, name: { value: 'unrelated' } });
  assert.deepEqual(projectAccessibility(nodes, observed), []);
  assert.deepEqual(projectAccessibility([{ backendDOMNodeId: 1, name: { value: 'input child value' } }], [{ id: 'title', kind: 'status-text', backendDOMNodeId: 1 }]), []);
  assert.throws(() => projectAccessibility(Array(1001).fill({}), []));
  assert.throws(() => projectAccessibility([], Array(65).fill({})));
});
