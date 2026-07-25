import { loadProject } from "../../../../services/projects";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  const { projectId } = await context.params;
  return Response.json(await loadProject(projectId));
}
