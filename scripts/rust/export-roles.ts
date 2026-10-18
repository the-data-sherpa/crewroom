import { writeFileSync } from 'node:fs';
import { defaultRoles } from '../../server/roles';
writeFileSync('crates/crewroom-core/src/default-roles.json', JSON.stringify(defaultRoles, null, 2) + '\n');
