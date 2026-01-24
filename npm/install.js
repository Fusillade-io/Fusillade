const fs = require('fs');
const path = require('path');
const https = require('https');
const { execSync } = require('child_process');
const packageJson = require('./package.json');

const VERSION = packageJson.version;
const PLATFORM = process.platform;
const ARCH = process.arch;

const BIN_DIR = path.join(__dirname, 'bin');
const BIN_NAME = PLATFORM === 'win32' ? 'fusillade-bin.exe' : 'fusillade-bin';
const BIN_PATH = path.join(BIN_DIR, BIN_NAME);

const MAPPING = {
    'darwin-x64': 'fusillade-macos-x64',
    'darwin-arm64': 'fusillade-macos-arm64',
    'linux-x64': 'fusillade-linux-x64',
    'win32-x64': 'fusillade-windows-x64.exe'
};

const key = `${PLATFORM}-${ARCH}`;
const artifactName = MAPPING[key];

if (!artifactName) {
    console.error(`Unsupported platform/architecture: ${key}`);
    process.exit(1);
}

const url = `https://github.com/Fusillade-io/Fusillade/releases/download/v${VERSION}/${artifactName}`;

console.log(`Downloading Fusillade v${VERSION} for ${key}...`);
console.log(`URL: ${url}`);

const file = fs.createWriteStream(BIN_PATH);

function download(downloadUrl) {
    https.get(downloadUrl, (response) => {
        if (response.statusCode === 302 || response.statusCode === 301) {
            download(response.headers.location);
            return;
        }

        if (response.statusCode !== 200) {
            console.error(`Failed to download binary. Status code: ${response.statusCode}`);
            console.error(`URL: ${downloadUrl}`);
            process.exit(1);
        }

        response.pipe(file);

        file.on('finish', () => {
            file.close(() => {
                console.log('Download completed.');
                if (PLATFORM !== 'win32') {
                    try {
                        fs.chmodSync(BIN_PATH, 0o755);
                        console.log('Made binary executable.');
                    } catch (err) {
                        console.error('Failed to make binary executable:', err);
                    }
                }
            });
        });
    }).on('error', (err) => {
        fs.unlink(BIN_PATH, () => { });
        console.error('Error downloading binary:', err.message);
        process.exit(1);
    });
}

// Ensure bin directory exists
if (!fs.existsSync(BIN_DIR)) {
    fs.mkdirSync(BIN_DIR, { recursive: true });
}

download(url);
