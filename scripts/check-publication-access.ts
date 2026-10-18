import { loadPublicationConnections } from '../server/publication-connection';
import { BitbucketClient, BitbucketUnavailable } from '../server/bitbucket-client';

const connections = loadPublicationConnections(process.argv[2] ?? '.crewroom/bitbucket.json');
if (!connections.length) throw new Error('Publication metadata is not configured.');
let failed = false;
for (const connection of connections) {
  try {
    const repository = await new BitbucketClient(connection).verifyRepository();
    console.log(JSON.stringify({ available: true, repository: repository.full_name, uuid: repository.uuid, providerWrites: 0 }));
  } catch (error) {
    failed = true; console.log(JSON.stringify({ available: false, repository: `${connection.repository.workspace}/${connection.repository.slug}`,
      reason: error instanceof BitbucketUnavailable ? error.message : 'Repository validation failed.', providerWrites: 0 }));
  }
}
process.exitCode = failed ? 1 : 0;
