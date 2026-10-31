import { expect, it } from 'vitest';
import { isRequest } from '../src/protocol';
it('rejects malformed and oversized messages', () => {
  expect(isRequest({ type: 'boot', profile: 'vanilla' })).toBe(true);
  expect(isRequest({ type: 'boot', profile: '../etc' })).toBe(false);
  expect(isRequest({ type: 'input', bytes: new Uint8Array(65537) })).toBe(false);
  expect(isRequest(null)).toBe(false);
  expect(isRequest({ type: 'import', bytes: new Uint8Array(1) })).toBe(false);
});
