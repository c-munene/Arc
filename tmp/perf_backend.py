from aiohttp import web
import argparse

parser = argparse.ArgumentParser()
parser.add_argument('--port', type=int, required=True)
args = parser.parse_args()

async def handle(request: web.Request) -> web.Response:
    return web.Response(text='ok', headers={'x-backend-port': str(args.port)})

app = web.Application()
app.router.add_route('*', '/{tail:.*}', handle)
web.run_app(app, host='127.0.0.1', port=args.port, access_log=None)
