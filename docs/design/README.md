# Design — Telinhas

Ponto de partida visual: projeto Stitch "MultiStream Screen Hub" (ID `4987780605515654660`). Os arquivos baixados estão em `stitch/` (HTML e JPG de cada tela).

**O Stitch é só referência visual (DEC-30).** Paleta, componentes e composição servem de base. Funcionamento e textos seguem `ESPECIFICACAO.md`.

## Referência visual aproveitada

- Tema escuro, índigo como cor de ação e verde como cor de "ao vivo"/conectado.
- Cartões arredondados, fundo com leve degradê.
- Barra flutuante de controles na sala: transmitir, câmera, microfone, qualidade, ajustes, sair.
- Painel lateral com a lista de transmissões e volume de cada uma.
- Modal de configuração de transmissão com abas de origem (telas / janelas).

## Telas

### 1. Início (`stitch/01-inicio.*`)

Duas ações: criar sala e entrar em sala (DEC-20).

| Mockup | Telinhas |
|--------|----------|
| Texto de marketing, selos, 4 passos, faixa de estatísticas, rodapé | Removidos (DEC-31) |
| Código `NEON-7842` | Código de 8 caracteres `XXXX-XXXX` (DEC-07) |
| "Expira após todos saírem" | "Expira 5 min após a sala esvaziar" (DEC-23) |
| Qualidade inicial e capacidade na criação da sala | Removidos. Qualidade é da transmissão (DEC-04) |
| "Exigir senha opcional de acesso" | Vira "Exigir aprovação para entrar", desligado por padrão (DEC-08, DEC-32) |
| Campo de código ou link | Campo de código (RF-02) |
| Modo "Assistindo / Transmitindo" | Mantido (DEC-35) |
| — | **Falta:** pedir apelido no primeiro uso e permitir trocá-lo (DEC-14) |

### 2. Sala ao vivo (`stitch/02-sala-ao-vivo.*`)

| Mockup | Telinhas |
|--------|----------|
| Aba "Chat Efêmero" | Removida (DEC-13) |
| Grid dinâmico / Foco-Cinema / 50-50 | Só Grid dinâmico e Foco (DEC-33) |
| Volume por feed | Mantido, local (DEC-34) |
| Bitrate total, perda de pacotes, latência média | Mantidos como dados reais. Latência por participante (DEC-19) |
| "Codec ativo VP9 / Opus" | Mostrar o codec real. O padrão é H.264 (DEC-15) |
| "Convidar amigo" | Copiar o código da sala (RF-01) |
| Menu de navegação entre páginas | Removido (DEC-20) |
| — | **Falta:** seletor "original / econômica" por transmissão assistida (DEC-04) |
| — | **Falta:** estimativa de banda de upload de quem transmite, com aviso acima de 12 pessoas (DEC-03, DEC-24) |
| — | **Falta:** pop-up de aprovação de entrada para o administrador (DEC-08) |
| — | **Falta:** indicação de quem é administrador (DEC-22) |
| — | **Falta:** estado "reconectando" (DEC-10) |
| — | **Falta:** aviso de encode por software (DEC-15) |
| — | **Falta:** botão para trocar a janela com a transmissão ativa (DEC-11) |
| — | **Falta:** webcam como picture-in-picture separado, que o espectador posiciona (DEC-12) |

### 3. Configurar transmissão (`stitch/03-configurar-transmissao.*`)

Modal sobre a sala (DEC-36).

| Mockup | Telinhas |
|--------|----------|
| Abas "Telas inteiras" e "Janelas de aplicativos" | Mantidas |
| Resolução 720p/1080p/1440p e 30/60 FPS fixos | Opções vindas do hardware e da fonte (DEC-36) |
| "Webcam sobreposta, insere sua imagem sobre a captura" | Webcam como vídeo separado em PiP (DEC-12). Texto a corrigir |
| "Roteamento de Áudio" com áudio do sistema e microfone | Mantido. Microfone desligado por padrão (DEC-16) |
| — | **Falta:** escolher aplicativos a excluir do áudio, ao capturar tela inteira (DEC-16) |
| — | **Falta:** opção de capturar o mouse, com os padrões da DEC-17 |
| — | **Falta:** banda de upload estimada para a configuração escolhida (DEC-03) |
| — | **Falta:** aviso quando só houver encode por software (DEC-15) |
| Rodapé "Status dos Relays", "WebRTC Health" | Removido (DEC-31) |
