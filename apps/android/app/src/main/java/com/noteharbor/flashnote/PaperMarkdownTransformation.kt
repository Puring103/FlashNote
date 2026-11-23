package com.noteharbor.flashnote

import androidx.compose.ui.text.*
import androidx.compose.ui.text.font.*
import androidx.compose.ui.text.input.*
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.sp

/** Presentation only: persisted Markdown and cursor offsets remain unchanged. */
internal class PaperMarkdownTransformation : VisualTransformation {
 override fun filter(text: AnnotatedString): TransformedText {
  val raw=text.text
  val hidden=BooleanArray(raw.length)
  val replacements=mutableMapOf<Int,Char>()
  val styles=mutableListOf<Triple<Int,Int,SpanStyle>>()
  fun hide(start:Int,end:Int) {for(i in start until end.coerceAtMost(raw.length)) hidden[i]=true}
  Regex("(?m)^(#{1,6}) (.+)$").findAll(raw).forEach {m ->
   val start=m.range.first; val prefix=m.groupValues[1].length+1
   hide(start,start+prefix)
   styles.add(Triple(start,m.range.last+1,SpanStyle(fontWeight=FontWeight.SemiBold,fontSize=if(prefix<4) 19.sp else 17.sp)))
  }
  Regex("\\*\\*([^*\\n]+)\\*\\*").findAll(raw).forEach {m ->
   hide(m.range.first,m.range.first+2);hide(m.range.last-1,m.range.last+1)
   styles.add(Triple(m.range.first,m.range.last+1,SpanStyle(fontWeight=FontWeight.SemiBold)))
  }
  Regex("(?<!\\*)\\*([^*\\n]+)\\*(?!\\*)").findAll(raw).forEach {m ->
   hide(m.range.first,m.range.first+1);hide(m.range.last,m.range.last+1)
   styles.add(Triple(m.range.first,m.range.last+1,SpanStyle(fontStyle=FontStyle.Italic)))
  }
  Regex("(?m)^[-*] \\[([ xX])\\] ").findAll(raw).forEach {m ->
   val start=m.range.first
   replacements[start]=if(m.groupValues[1]==" ") '□' else '✓'
   hide(start+1,m.range.last)
  }
  Regex("(?m)^[-*] (?!\\[)").findAll(raw).forEach {m ->replacements[m.range.first]='•'}
  Regex("\\[素材\\]\\(attachment:[^)]*\\)").findAll(raw).forEach {m ->hide(m.range.first,m.range.last+1)}
  val originalOffsets=IntArray(raw.length+1)
  val transformedOffsets=mutableListOf(0)
  val output=StringBuilder()
  for(i in raw.indices) {
   originalOffsets[i]=output.length
   if(hidden[i]) transformedOffsets[output.length]=i+1
   else {output.append(replacements[i] ?: raw[i]);transformedOffsets.add(i+1)}
  }
  originalOffsets[raw.length]=output.length
  val annotated=buildAnnotatedString {
   append(output.toString())
   styles.forEach {(start,end,style) -> if(originalOffsets[start]<originalOffsets[end]) addStyle(style,originalOffsets[start],originalOffsets[end])}
  }
  return TransformedText(annotated,object:OffsetMapping {
   override fun originalToTransformed(offset:Int)=originalOffsets[offset.coerceIn(0,raw.length)]
   override fun transformedToOriginal(offset:Int)=transformedOffsets[offset.coerceIn(0,output.length)]
  })
 }
}
