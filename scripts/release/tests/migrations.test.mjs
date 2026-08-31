import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repository = resolve(import.meta.dirname, '../../..');
const migrationsDirectory = resolve(repository, 'migrations');

const immutableMigrations = new Map([
  ['001_create_users.sql', '32ff83d03803da2b0290c65764121a4bb39d6b5f179382e41bc133e01ea01a46b576ccccee99842515063ec493bcedd9'],
  ['002_create_sessions.sql', 'a762dc45bc21523abce4c22d439b7577e5859e6a162e87d3aec6139205993045929639515c0efe1354acaefafddef907'],
  ['003_create_api_keys.sql', 'd853776f9bd95ca0d37d00091ef747e87dafdd0f84ee546d27bb5e6c1ea130ad5b92f034fae5ba27675390218d4a9baf'],
  ['004_create_bookmarks.sql', '9723d6b266d72f02043529a460aeb17cbf4fb04ed2c17ab93c2141df19ee728e92cd1e257d8493b653749d8ce692529a'],
  ['005_create_user_llm_settings.sql', '1cc577abf5b80edf1ce47a425a07fb29375a59082baacccba445fb8c5eff3411209c29dbc47ecc515f3ef6a4022fcd3d'],
  ['006_add_password_hash.sql', 'bc2fe8f63e4cd2147163690b67c6c79e4a9c59b907ec6ce2b86144d1d099776589c18c48a935a6a52b31ab12d6170644'],
  ['007_add_user_role_and_deactivated_at.sql', '895969000babcfa61c4e15550221589427a6fc6ba3195ba3e7dee3780d19ab6fa7c43789524b3bf0eea8ef0ac7470f19'],
  ['008_create_invites.sql', '3b68bde5019a61a3497c44e622da64ebd74fcf72e497283cc3a96b4133be36f7f224725d3b2d704a158ebdecd0232cbb'],
  ['009_add_override_image_url.sql', 'e38447b9ff9e8b92c1d0ad3cc2cb3ba561cca22d17b3ca8f3de31646691b5b7e4c8e4a60a5f035918bfdda34f3273060'],
  ['010_add_bookmark_idempotency.sql', 'b8b5ca193373721b57884144b90627968b19cb5ea411c049d3f31e17cd1c28abca9a838eab46907be5294e24822c89e8'],
  ['011_add_image_generation_settings.sql', '49c7fa7db3d40b44e1e307ffb91b9a53233b53ee7f76283569fe390609b77728feb2a5ce86282045c8c93cd8d3d0257a'],
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
