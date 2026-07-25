import { createProject, listProjects } from "../../../services/projects";

export async function GET() {
  return Response.json(await listProjects());
}

export async function POST(request: Request) {
  return Response.json(await createProject(await request.json()), {
    status: 201,
  });
}
