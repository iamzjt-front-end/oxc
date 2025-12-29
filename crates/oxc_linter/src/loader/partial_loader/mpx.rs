use memchr::memmem::{Finder, FinderRev};

use oxc_span::SourceType;

use super::{
    COMMENT_END, COMMENT_START, JavaScriptSource, SCRIPT_END, SCRIPT_START,
    find_script_closing_angle, find_script_start,
};

pub struct MpxPartialLoader<'a> {
    source_text: &'a str,
}

impl<'a> MpxPartialLoader<'a> {
    pub fn new(source_text: &'a str) -> Self {
        Self { source_text }
    }

    pub fn parse(self) -> Vec<JavaScriptSource<'a>> {
        self.parse_scripts()
    }

    /// MPX files can contain multiple `<script>` blocks.
    /// We need to skip `<script type="application/json">` which is used for page config.
    fn parse_scripts(&self) -> Vec<JavaScriptSource<'a>> {
        let mut results = vec![];
        let mut pointer = 0;

        while let Some(result) = self.parse_script(&mut pointer) {
            results.push(result);
        }

        results
    }

    fn parse_script(&self, pointer: &mut usize) -> Option<JavaScriptSource<'a>> {
        let script_start_finder = Finder::new(SCRIPT_START);
        let comment_start_finder = FinderRev::new(COMMENT_START);
        let comment_end_finder = Finder::new(COMMENT_END);

        loop {
            // find opening "<script"
            *pointer += find_script_start(
                self.source_text,
                *pointer,
                &script_start_finder,
                &comment_start_finder,
                &comment_end_finder,
            )?;

            // skip `<script-` (e.g. <script-view />)
            if !self.source_text[*pointer..].starts_with([' ', '>']) {
                continue;
            }

            // find closing ">"
            let offset = find_script_closing_angle(self.source_text, *pointer)?;
            let content = &self.source_text[*pointer..*pointer + offset];

            // parse `lang` attribute, or detect JSON script
            let lang = Self::extract_lang_attribute(content);
            let Ok(mut source_type) = SourceType::from_extension(lang) else {
                *pointer += offset + 1;
                continue;
            };

            if !lang.contains('x') {
                source_type = source_type.with_standard(true);
            }

            *pointer += offset + 1;
            let js_start = *pointer;

            // find "</script>"
            let script_end_finder = Finder::new(SCRIPT_END);
            let end_offset = script_end_finder.find(&self.source_text.as_bytes()[*pointer..])?;
            let js_end = *pointer + end_offset;
            *pointer += end_offset + SCRIPT_END.len();

            let source_text = &self.source_text[js_start..js_end];
            #[expect(clippy::cast_possible_truncation)]
            return Some(JavaScriptSource::partial(source_text, source_type, js_start as u32));
        }
    }

    fn extract_lang_attribute(content: &str) -> &str {
        let content = content.trim();

        let Some(lang_index) = content.find("lang") else {
            return "mjs";
        };

        let mut rest = content[lang_index + 4..].trim_start();

        if !rest.starts_with('=') {
            return "mjs";
        }

        rest = rest[1..].trim_start();

        let first_char = rest.chars().next();

        match first_char {
            Some('"' | '\'') => {
                let quote = first_char.unwrap();
                rest = &rest[1..];
                match rest.find(quote) {
                    Some(end) => &rest[..end],
                    None => "mjs",
                }
            }
            Some(_) => match rest.find(|c: char| c.is_whitespace() || c == '>') {
                Some(end) => &rest[..end],
                None => rest,
            },
            None => "mjs",
        }
    }
}

#[cfg(test)]
mod test {
    use oxc_span::SourceType;

    use super::{JavaScriptSource, MpxPartialLoader};

    fn parse_mpx(source_text: &str) -> JavaScriptSource<'_> {
        let sources = MpxPartialLoader::new(source_text).parse();
        *sources.first().unwrap()
    }

    fn parse_mpx_all(source_text: &str) -> Vec<JavaScriptSource<'_>> {
        MpxPartialLoader::new(source_text).parse()
    }

    // ==================== Basic Parsing ====================

    #[test]
    fn test_parse_mpx_one_line() {
        let source_text = r#"
        <template>
          <view>hello world</view>
        </template>
        <script> console.log("hi") </script>
        "#;

        let result = parse_mpx(source_text);
        assert_eq!(result.source_text, r#" console.log("hi") "#);
    }

    #[test]
    fn test_basic_script() {
        let source_text = r#"
        <template>
          <view>hello</view>
        </template>
        <script>
        console.log("hi")
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("console.log"));
    }

    // ==================== TypeScript Support ====================

    #[test]
    fn test_typescript_double_quote() {
        let source_text = r#"
        <script lang="ts">
            const x: number = 1;
        </script>
        "#;

        let result = parse_mpx(source_text);
        assert_eq!(result.source_type, SourceType::ts());
        assert!(result.source_text.contains("const x: number = 1"));
    }

    #[test]
    fn test_typescript_single_quote() {
        let source_text = r"
        <script lang='ts'>
            const y: string = 'hello';
        </script>
        ";

        let result = parse_mpx(source_text);
        assert_eq!(result.source_type, SourceType::ts());
    }

    #[test]
    fn test_typescript_with_spaces() {
        let source_text = r#"
        <script lang = "ts" >
            1/1
        </script>
        "#;

        let result = parse_mpx(source_text);
        assert_eq!(result.source_type, SourceType::ts());
        assert_eq!(result.source_text.trim(), "1/1");
    }

    #[test]
    fn test_tsx() {
        let source_text = r#"
        <script lang="tsx">
            const el = <div>hello</div>;
        </script>
        "#;

        let result = parse_mpx(source_text);
        assert_eq!(result.source_type, SourceType::tsx());
    }

    // ==================== JSON Config Scripts (MPX specific) ====================
    // MPX JSON config scripts should also be parsed (not skipped)

    #[test]
    fn test_json_type_also_parsed() {
        let source_text = r#"
        <script>
        const a = 1;
        </script>
        <script type="application/json">
        {
          "usingComponents": {}
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        // Both scripts should be parsed
        assert_eq!(sources.len(), 2);
        assert!(sources[0].source_text.contains("const a = 1"));
        assert!(sources[1].source_text.contains("usingComponents"));
    }

    #[test]
    fn test_json_name_also_parsed() {
        let source_text = r#"
        <script>
        const b = 2;
        </script>
        <script name="json">
        {
          "navigationBarTitleText": "test"
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 2);
        assert!(sources[0].source_text.contains("const b = 2"));
        assert!(sources[1].source_text.contains("navigationBarTitleText"));
    }

    #[test]
    fn test_only_json_script() {
        let source_text = r#"
        <template><view></view></template>
        <script type="application/json">
        { "usingComponents": {} }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        // JSON script should also be parsed
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("usingComponents"));
    }

    #[test]
    fn test_json_type_single_quote() {
        let source_text = r"
        <script type='application/json'>
        { 'key': 'value' }
        </script>
        ";

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("key"));
    }

    #[test]
    fn test_json_name_single_quote() {
        let source_text = r"
        <script name='json'>
        { 'config': true }
        </script>
        ";

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("config"));
    }

    #[test]
    fn test_json_complex_config() {
        let source_text = r#"
        <script type="application/json">
        {
          "usingComponents": {
            "van-button": "@vant/weapp/button/index",
            "van-cell": "@vant/weapp/cell/index"
          },
          "navigationBarTitleText": "首页",
          "enablePullDownRefresh": true,
          "backgroundTextStyle": "dark"
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("van-button"));
        assert!(sources[0].source_text.contains("navigationBarTitleText"));
        assert!(sources[0].source_text.contains("enablePullDownRefresh"));
    }

    #[test]
    fn test_json_with_chinese() {
        let source_text = r#"
        <script type="application/json">
        {
          "navigationBarTitleText": "微信小程序",
          "tabBar": {
            "list": [
              { "text": "首页", "pagePath": "pages/index/index" },
              { "text": "我的", "pagePath": "pages/mine/mine" }
            ]
          }
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("微信小程序"));
        assert!(sources[0].source_text.contains("首页"));
        assert!(sources[0].source_text.contains("我的"));
    }

    #[test]
    fn test_json_empty_object() {
        let source_text = r#"
        <script type="application/json">
        {}
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_text.trim(), "{}");
    }

    #[test]
    fn test_json_with_array() {
        let source_text = r#"
        <script type="application/json">
        {
          "pages": [
            "pages/index/index",
            "pages/logs/logs",
            "pages/user/user"
          ]
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("pages/index/index"));
        assert!(sources[0].source_text.contains("pages/logs/logs"));
    }

    #[test]
    fn test_json_with_nested_objects() {
        let source_text = r##"
        <script type="application/json">
        {
          "window": {
            "backgroundTextStyle": "light",
            "navigationBarBackgroundColor": "#fff",
            "navigationBarTitleText": "Demo",
            "navigationBarTextStyle": "black"
          },
          "style": "v2",
          "sitemapLocation": "sitemap.json"
        }
        </script>
        "##;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("window"));
        assert!(sources[0].source_text.contains("backgroundTextStyle"));
    }

    #[test]
    fn test_json_with_boolean_and_numbers() {
        let source_text = r#"
        <script type="application/json">
        {
          "enablePullDownRefresh": true,
          "disableScroll": false,
          "onReachBottomDistance": 50,
          "initialRenderingCache": "static"
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("true"));
        assert!(sources[0].source_text.contains("false"));
        assert!(sources[0].source_text.contains("50"));
    }

    #[test]
    fn test_json_minified() {
        let source_text = r#"<script type="application/json">{"a":1,"b":"test","c":true}</script>"#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_text, r#"{"a":1,"b":"test","c":true}"#);
    }

    #[test]
    fn test_multiple_json_scripts() {
        // Some MPX files might have multiple JSON config sections
        let source_text = r#"
        <script type="application/json">
        { "config1": true }
        </script>
        <script name="json">
        { "config2": false }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 2);
        assert!(sources[0].source_text.contains("config1"));
        assert!(sources[1].source_text.contains("config2"));
    }

    #[test]
    fn test_js_and_json_mixed() {
        let source_text = r#"
        <script>
        import { createPage } from '@mpxjs/core';
        createPage({
          data: { count: 0 }
        });
        </script>
        <script type="application/json">
        {
          "usingComponents": {},
          "navigationBarTitleText": "计数器"
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 2);
        assert!(sources[0].source_text.contains("createPage"));
        assert!(sources[1].source_text.contains("usingComponents"));
    }

    #[test]
    fn test_ts_and_json_mixed() {
        let source_text = r#"
        <script lang="ts">
        interface PageData {
          count: number;
        }
        const data: PageData = { count: 0 };
        </script>
        <script type="application/json">
        { "navigationBarTitleText": "TypeScript Page" }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].source_type, SourceType::ts());
        assert!(sources[0].source_text.contains("PageData"));
        assert!(sources[1].source_text.contains("TypeScript Page"));
    }

    #[test]
    fn test_json_with_special_characters() {
        let source_text = r#"
        <script type="application/json">
        {
          "path": "pages/detail/detail?id=123&name=test",
          "url": "https://example.com/api?foo=bar",
          "regex": "\\d+\\.\\d+"
        }
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].source_text.contains("id=123&name=test"));
        assert!(sources[0].source_text.contains("https://example.com"));
    }

    #[test]
    fn test_json_position_tracking() {
        let source_text = r#"<script type="application/json">{"key": "value"}</script>"#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        // The start position should be right after the opening tag
        assert_eq!(sources[0].start, 32); // length of `<script type="application/json">`
    }

    // ==================== Multiple Scripts ====================

    #[test]
    fn test_multiple_scripts() {
        let source_text = r"
        <template></template>
        <script>a</script>
        <script>b</script>
        ";
        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].source_text, "a");
        assert_eq!(sources[1].source_text, "b");
    }

    #[test]
    fn test_script_with_json_config() {
        let source_text = r#"
        <script>a</script>
        <script type="application/json">{}</script>
        <script>b</script>
        "#;
        let sources = parse_mpx_all(source_text);
        // All 3 scripts should be parsed
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].source_text, "a");
        assert_eq!(sources[1].source_text, "{}");
        assert_eq!(sources[2].source_text, "b");
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_no_script() {
        let source_text = r"
            <template><view></view></template>
        ";

        let sources = parse_mpx_all(source_text);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_syntax_error_unclosed_script() {
        let source_text = r"
        <script>
            console.log('error')
        ";
        let sources = parse_mpx_all(source_text);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_unicode() {
        let source_text = r"
        <script>
        let 日历 = '2000年';
        const t = {
            'zh-CN': {
                calendar: '日历',
                tiledDisplay: '平铺展示',
            },
        };
        </script>
        ";

        let result = parse_mpx(source_text);
        assert!(result.source_text.contains("日历"));
        assert!(result.source_text.contains("平铺展示"));
    }

    #[test]
    fn test_script_in_template() {
        // <script-view /> should not be treated as a script tag
        let source_text = r"
        <template><script-view /></template>
        <script>a</script>
        ";
        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_text, "a");
    }

    #[test]
    fn test_closing_character_inside_attribute() {
        let source_text = r"
        <script description='PI > 5'>a</script>
        ";

        let result = parse_mpx(source_text);
        assert_eq!(result.source_text, "a");
    }

    #[test]
    fn test_script_inside_comment() {
        let source_text = r"
        <!-- <script>a</script> -->
        <!-- <script> -->
        <script>b</script>
        ";

        let result = parse_mpx(source_text);
        assert_eq!(result.source_text, "b");
        assert_eq!(result.start, 79);
    }

    #[test]
    fn test_escape_string() {
        let source_text = r"
        <script>
            a.replace(/&#39;/g, '\''))
        </script>
        <template> </template>
        ";

        let result = parse_mpx(source_text);
        assert!(!result.source_type.is_typescript());
        assert_eq!(result.source_text.trim(), r"a.replace(/&#39;/g, '\''))");
    }

    #[test]
    fn test_multi_level_template_literal() {
        let source_text = r"
        <script>
            `a${b( `c \`${d}\``)}`
        </script>
        ";

        let result = parse_mpx(source_text);
        assert_eq!(result.source_text.trim(), r"`a${b( `c \`${d}\``)}`");
    }

    #[test]
    fn test_brace_with_regex_in_template_literal() {
        let source_text = r"
        <script>
            `${/{/}`
        </script>
        ";

        let result = parse_mpx(source_text);
        assert_eq!(result.source_text.trim(), r"`${/{/}`");
    }

    // ==================== Lang Attribute Variations ====================

    #[test]
    fn lang() {
        let cases = [
            ("<script>debugger</script>", Some(SourceType::mjs())),
            ("<script lang = 'tsx' >debugger</script>", Some(SourceType::tsx())),
            (r#"<script lang = "cjs" >debugger</script>"#, Some(SourceType::cjs())),
            ("<script lang=tsx>debugger</script>", Some(SourceType::tsx())),
            ("<script lang = 'xxx'>debugger</script>", None),
            (r#"<script lang = "xxx">debugger</script>"#, None),
            ("<script lang='xxx'>debugger</script>", None),
            (r#"<script lang="xxx">debugger</script>"#, None),
        ];

        for (source_text, source_type) in cases {
            let sources = parse_mpx_all(source_text);
            if let Some(expected) = source_type {
                assert_eq!(sources.len(), 1, "Failed for: {source_text}");
                assert_eq!(sources[0].source_type, expected, "Failed for: {source_text}");
            } else {
                assert_eq!(sources.len(), 0, "Failed for: {source_text}");
            }
        }
    }

    // ==================== MPX Specific: wxs script ====================

    #[test]
    fn test_wxs_script_skipped() {
        // wxs scripts have different syntax, should be handled separately if needed
        // For now, we just test that regular scripts work alongside
        let source_text = r#"
        <script>
        const a = 1;
        </script>
        "#;

        let sources = parse_mpx_all(source_text);
        assert_eq!(sources.len(), 1);
    }
}
