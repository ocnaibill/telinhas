# Telinhas — Como distribuir (Windows, beta)

Para os amigos do beta: um instalador que não pede administrador, que se atualiza sozinho toda vez que o app abre, e uma versão com o servidor já embutido (ninguém digita o endereço).

## Como funciona

- **Instalador** (`installer/telinhas.iss`, Inno Setup 6): instala em `%LocalAppData%\Programs\Telinhas` com o `.exe` e as DLLs do FFmpeg, cria o atalho no menu Iniciar e, se a pessoa quiser, na área de trabalho. Não pede permissão de administrador.
- **Atualização**: ao abrir, o app lê `latest.json` no endereço embutido no build. Se a versão for mais nova, baixa o instalador, confere o SHA-256 e o roda em modo silencioso; o app fecha, o instalador troca os arquivos e abre o app de novo. Se o servidor não responde, o app abre normalmente.
- **Onde ficam os arquivos**: o servidor de sinalização serve uma pasta em `/dl` (variável `TELINHAS_DOWNLOADS_DIR`), pelo mesmo endereço do cloudflared. A pasta é só leitura para o servidor (RNF-20).
- **Servidor embutido**: o build feito com `-Server` já sai apontando para ele, e o campo de endereço some da tela inicial. `--server URL` ainda funciona, para desenvolvimento.
- **Microfone e câmera** não aparecem no beta (`MIC_AND_CAMERA` em `app/src/app.rs`).

## Preparar o computador que gera o instalador (uma vez)

Rust, Visual Studio Build Tools, LLVM (`LIBCLANG_PATH`), o FFmpeg compartilhado (`FFMPEG_DIR`), `cmake` e o Inno Setup:

```powershell
winget install Kitware.CMake
winget install JRSoftware.InnoSetup
```

## Gerar uma versão

1. Aumente a versão em `Cargo.toml` (`[workspace.package] version`), por exemplo `0.1.1`. O atualizador só instala versões **maiores** que a instalada.
2. Rode o script, com o endereço de onde os arquivos serão servidos (sem o nome do arquivo) e o servidor dos amigos:

```powershell
.\scripts\build-release.ps1 -BaseUrl https://SEU-HOST/dl -Server wss://SEU-HOST/ws -Notes "o que mudou"
```

3. O script escreve em `dist\`: `Telinhas-Setup-<versão>.exe`, `Telinhas-Setup.exe` (o mesmo arquivo com um nome que nunca muda) e `latest.json`. Copie os três para a pasta servida em `/dl`.
4. Quem já tem o app recebe a versão na próxima vez que abrir. Quem vai instalar pela primeira vez baixa `https://SEU-HOST/dl/Telinhas-Setup.exe`.

## Ligar o servidor

No homelab, onde o servidor de sinalização roda, aponte a pasta de downloads:

```bash
TELINHAS_DOWNLOADS_DIR=/caminho/da/pasta telinhas-signaling
```

Ao subir, ele imprime `downloads served at /dl from ...`. Sem a variável, `/dl` não existe.

## Limites conhecidos

- **O instalador não é assinado.** O Windows SmartScreen vai avisar "o Windows protegeu seu computador" na primeira instalação: é preciso clicar em "Mais informações" e "Executar assim mesmo". Assinar exige um certificado de assinatura de código (pago).
- **Smart App Control (Windows 11) bloqueia o app** (DEC-66). Com ele ligado, o Windows recusa as DLLs não assinadas ("Imagem incorreta", erro `0xc0e90002`). Confirmado no PC de um amigo: hash da DLL correto, `VerifiedAndReputablePolicyState = 1`. A saída no beta é desligá-lo em Segurança do Windows > Controle de aplicativos e do navegador > Configurações do Smart App Control. Aviso: em versões do Windows em que isso não pode ser desfeito sem reinstalar, quem desliga não consegue religar. A correção de verdade é assinar o código.
- **A atualização confere o hash, mas não uma assinatura.** O hash vem do mesmo servidor que o arquivo, então protege de download corrompido ou truncado, e de um manifesto que aponte para outro servidor, mas não de quem controlar o servidor. Para o beta com amigos é aceitável; antes de abrir para o público, o próximo passo é assinar o instalador com uma chave que só o mantenedor tem e verificar a assinatura no app.
- **As DLLs do FFmpeg são LGPL.** `installer/THIRD-PARTY-NOTICES.txt` vai junto no instalador. A revisão jurídica dos termos de redistribuição é do mantenedor.
- Só Windows. No macOS e no Linux (M7) o instalador e a atualização serão outros.
