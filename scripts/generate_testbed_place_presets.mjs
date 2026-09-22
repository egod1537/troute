#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const sourceDirectory = resolve(repositoryRoot, 'tests/fixtures/places');
const defaultOutput = resolve(
  repositoryRoot,
  process.env.TCACHE_REPOSITORY ?? '../tcache',
  'route/testbed/matrix/place-fixtures.generated.ts',
);

const argumentsList = process.argv.slice(2);
const check = argumentsList.includes('--check');
const outputFlagIndex = argumentsList.indexOf('--output');
const outputPath =
  outputFlagIndex >= 0
    ? resolve(repositoryRoot, argumentsList[outputFlagIndex + 1])
    : defaultOutput;

if (outputFlagIndex >= 0 && !argumentsList[outputFlagIndex + 1]) {
  throw new Error('--output requires a path');
}

const citySpecifications = [
  { id: 'tokyo', languageCode: 'ja', fixture: 'tokyo_10_places.json' },
  { id: 'seoul', languageCode: 'ko', fixture: 'seoul_10_places.json' },
];

const cities = await Promise.all(
  citySpecifications.map(async ({ id, languageCode, fixture }) => {
    const parsed = JSON.parse(
      await readFile(resolve(sourceDirectory, fixture), 'utf8'),
    );
    return {
      id,
      name: parsed.name.replace(/ 10 Places$/, ''),
      regionCode: parsed.region,
      languageCode,
      timezone: parsed.timezone,
      defaultStartTime: parsed.default_start_time,
      verifiedAt: parsed.verified_at,
      locations: parsed.locations.map(
        ({ id: locationId, name, place_id, address, latitude, longitude }) => ({
          id: locationId,
          name,
          placeId: place_id,
          address,
          latitude,
          longitude,
        }),
      ),
    };
  }),
);

const unformatted = `// This file is generated from troute/tests/fixtures/places/*_10_places.json.
// Do not edit it by hand. Run: node scripts/generate_testbed_place_presets.mjs

export interface GeneratedPlaceFixtureLocation {
  id: string;
  name: string;
  placeId: string;
  address?: string;
  latitude?: number;
  longitude?: number;
}

export interface GeneratedPlaceFixtureCity {
  id: string;
  name: string;
  regionCode: string;
  languageCode: string;
  timezone: string;
  defaultStartTime: string;
  verifiedAt: string;
  locations: GeneratedPlaceFixtureLocation[];
}

export const GENERATED_PLACE_FIXTURE_CITIES: GeneratedPlaceFixtureCity[] = ${JSON.stringify(cities, null, 2)};
`;

const tcacheRoot = resolve(dirname(outputPath), '../../..');
const prettier = await import(
  pathToFileURL(resolve(tcacheRoot, 'node_modules/prettier/index.mjs')).href
).catch(() => {
  throw new Error(
    `Prettier is not installed in ${tcacheRoot}; run pnpm install in the tcache repository`,
  );
});
const generated = await prettier.format(unformatted, {
  ...(await prettier.resolveConfig(outputPath)),
  filepath: outputPath,
});

if (check) {
  const current = await readFile(outputPath, 'utf8').catch(() => '');
  if (current !== generated) {
    console.error(`Generated Testbed presets are stale: ${outputPath}`);
    process.exitCode = 1;
  }
} else {
  await writeFile(outputPath, generated, 'utf8');
  console.log(`Wrote ${outputPath}`);
}
