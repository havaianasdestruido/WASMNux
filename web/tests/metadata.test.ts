import 'fake-indexeddb/auto';
import { expect, it } from 'vitest';
import { putSnapshot, listSnapshots, getSnapshot, deleteSnapshot } from '../src/metadata';
it('stores and filters snapshot metadata per profile', async () => {
  await putSnapshot({ id: 'a', profile: 'vanilla', created: 1, diskHash: 'x', bytes: 10 });
  await putSnapshot({ id: 'b', profile: 'debian', created: 2, diskHash: 'y', bytes: 20 });
  expect((await listSnapshots('vanilla')).map((s) => s.id)).toEqual(['a']);
  expect((await getSnapshot('b'))?.profile).toBe('debian');
  await deleteSnapshot('a');
  expect(await listSnapshots('vanilla')).toEqual([]);
});
