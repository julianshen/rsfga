#!/usr/bin/env node

/**
 * Tuple Generation Script for RSFGA Load Tests
 *
 * Generates tuple datasets of various sizes:
 * - small: 10K tuples
 * - medium: 100K tuples
 * - large: 1M tuples
 *
 * Usage:
 *   node generate-tuples.js [size] [output-dir]
 *   node generate-tuples.js small ../k6/data/tuples/
 *   node generate-tuples.js all ../k6/data/tuples/
 */

const fs = require('fs');
const path = require('path');

// Configuration for each size
const CONFIG = {
  small: {
    users: 1000,
    documents: 100,
    folders: 20,
    groups: 10,
    membersPerGroup: 50,
    directAssignments: 0.6,
    conditionRate: 0.1,
    targetTuples: 10000,
  },
  medium: {
    users: 10000,
    documents: 1000,
    folders: 100,
    groups: 100,
    membersPerGroup: 50,
    directAssignments: 0.5,
    conditionRate: 0.2,
    targetTuples: 100000,
  },
  large: {
    users: 100000,
    documents: 10000,
    folders: 1000,
    groups: 500,
    membersPerGroup: 100,
    directAssignments: 0.4,
    conditionRate: 0.3,
    targetTuples: 1000000,
  },
};

const RELATIONS = ['viewer', 'editor', 'owner'];
const DEPARTMENTS = ['engineering', 'product', 'sales', 'marketing', 'hr'];

function generateTuples(config) {
  const tuples = [];

  // 1. Group memberships
  console.log('  Generating group memberships...');
  for (let g = 0; g < config.groups; g++) {
    for (let m = 0; m < config.membersPerGroup; m++) {
      const userId = (g * config.membersPerGroup + m) % config.users;
      tuples.push({
        user: `user:user_${userId}`,
        relation: 'member',
        object: `group:group_${g}`,
      });
    }
  }

  // 2. Folder hierarchy (5 levels)
  console.log('  Generating folder hierarchy...');
  const foldersPerLevel = Math.floor(config.folders / 5);
  for (let level = 1; level < 5; level++) {
    for (let f = 0; f < foldersPerLevel; f++) {
      const parentFolder = (level - 1) * foldersPerLevel + (f % foldersPerLevel);
      const childFolder = level * foldersPerLevel + f;
      if (childFolder < config.folders) {
        tuples.push({
          user: `folder:folder_${parentFolder}`,
          relation: 'parent',
          object: `folder:folder_${childFolder}`,
        });
      }
    }
  }

  // 3. Document-folder relationships
  console.log('  Generating document-folder relationships...');
  for (let d = 0; d < config.documents; d++) {
    tuples.push({
      user: `folder:folder_${d % config.folders}`,
      relation: 'parent',
      object: `document:doc_${d}`,
    });
  }

  // 4. Direct user assignments
  console.log('  Generating direct user assignments...');
  const directCount = Math.floor(config.targetTuples * config.directAssignments);
  for (let i = 0; i < directCount && tuples.length < config.targetTuples; i++) {
    const userId = i % config.users;
    const docId = Math.floor(Math.random() * config.documents);
    const relation = RELATIONS[i % RELATIONS.length];

    if (Math.random() < config.conditionRate) {
      // With condition
      tuples.push({
        user: `user:user_${userId}`,
        relation,
        object: `document:doc_${docId}`,
        condition: {
          name: 'department_condition',
          context: {
            dept: DEPARTMENTS[i % DEPARTMENTS.length],
          },
        },
      });
    } else {
      // Without condition
      tuples.push({
        user: `user:user_${userId}`,
        relation,
        object: `document:doc_${docId}`,
      });
    }
  }

  // 5. Group-based assignments
  console.log('  Generating group-based assignments...');
  while (tuples.length < config.targetTuples) {
    const groupId = Math.floor(Math.random() * config.groups);
    const docId = Math.floor(Math.random() * config.documents);
    const relation = RELATIONS[tuples.length % RELATIONS.length];

    tuples.push({
      user: `group:group_${groupId}#member`,
      relation,
      object: `document:doc_${docId}`,
    });
  }

  return tuples.slice(0, config.targetTuples);
}

function saveTuples(tuples, outputPath) {
  const data = {
    generated_at: new Date().toISOString(),
    count: tuples.length,
    tuples,
  };

  fs.writeFileSync(outputPath, JSON.stringify(data, null, 2));
  console.log(`  Saved ${tuples.length} tuples to ${outputPath}`);
}

function main() {
  const args = process.argv.slice(2);
  const size = args[0] || 'small';
  const outputDir = args[1] || path.join(__dirname, '..', 'k6', 'data', 'tuples');

  // Ensure output directory exists
  if (!fs.existsSync(outputDir)) {
    fs.mkdirSync(outputDir, { recursive: true });
  }

  const sizes = size === 'all' ? ['small', 'medium', 'large'] : [size];

  for (const s of sizes) {
    if (!CONFIG[s]) {
      console.error(`Unknown size: ${s}. Use small, medium, large, or all.`);
      process.exit(1);
    }

    console.log(`Generating ${s} dataset (${CONFIG[s].targetTuples} tuples)...`);
    const tuples = generateTuples(CONFIG[s]);

    const filename = `${s}-${CONFIG[s].targetTuples / 1000}k.json`;
    const outputPath = path.join(outputDir, filename);
    saveTuples(tuples, outputPath);
  }

  console.log('Done!');
}

main();
