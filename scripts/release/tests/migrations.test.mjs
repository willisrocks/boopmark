import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repository = resolve(import.meta.dirname, '../../..');
const migrationsDirectory = resolve(repository, 'migrations');

const immutableMigrations = new Map([
  ['012_add_openai_llm_settings.sql', '3a4efb37482d664351962d68b4a4b016894b8702c4b5293fa046c1ad54621190dfc53ed0ca9adadebd73b34ad53fc5c4'],
  ['013_restore_gemini_image_model.sql', '68999a360292926ab768d0fee9eecb865c7f2c83305a950f51ec132fd552abda106803251b8e4ab8f14f23516a7376e2'],
  ['014_restore_openai_image_model.sql', 'b46b31973f96a202783ea5d7925c38234b3d2aa94755b9896608ef240a919b2a18a2c3060e95a8405c8050b23200c99b'],
]);

test('migration numbers are unique and contiguous', async () => {
  const files = (await readdir(migrationsDirectory))
    .filter(file => /^\d{3}_.+\.sql$/.test(file))
    .sort();
  const numbers = files.map(file => Number.parseInt(file.slice(0, 3), 10));

  assert.deepEqual(numbers, Array.from({ length: numbers.length }, (_, index) => index + 1));
});

test('production migration history remains byte-for-byte immutable', async () => {
  for (const [file, expected] of immutableMigrations) {
    const contents = await readFile(resolve(migrationsDirectory, file));
    const actual = createHash('sha384').update(contents).digest('hex');
    assert.equal(actual, expected, `${file} checksum changed`);
  }
});
